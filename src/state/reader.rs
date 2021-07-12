use std::sync::{atomic::Ordering, Arc};

use crate::packet::{Packet, Request, Response};
use async_std::{
	channel::Sender,
	io::ReadExt,
	net::TcpStream,
	task::{spawn, JoinHandle},
};
use color_eyre::eyre::Result;
use deku::{prelude::DekuError, DekuContainerRead};
use futures::io::ReadHalf;
use log::{debug, trace, warn};

impl super::Worker {
	pub fn reader(
		self: Arc<Self>,
		mut gear_read: ReadHalf<TcpStream>,
		res_s: Sender<Response>,
		req_s: Sender<Request>,
	) -> JoinHandle<Result<()>> {
		spawn(async move {
			let mut packet = Vec::with_capacity(1024);
			'recv: loop {
				trace!("[{}] waiting for data", self.name);
				let mut buf = vec![0_u8; 1024];
				let len = ReadExt::read(&mut gear_read, &mut buf).await?;
				packet.extend(&buf[0..len]);
				trace!("[{}] received packet bytes: {:?}", self.name, &packet);

				'parse: loop {
					if packet.is_empty() {
						break 'parse;
					}

					match Packet::from_bytes((&packet, 0)) {
						Ok(((rest, _), pkt)) => {
							trace!("[{}] parsed packet: {:?}", self.name, &pkt);

							trace!("[{}] data left: {} bytes", self.name, rest.len());
							packet = rest.to_vec();

							if let Some(res @ Response::JobAssignUniq { .. }) = pkt.response {
								debug!("[{}] got a job assignment", self.name);
								res_s.send(res).await?;
							} else if let Some(Response::Noop) = pkt.response {
								if self.current_load.load(Ordering::Relaxed) < self.concurrency {
									debug!("[{}] got a noop, asking for work", self.name);
									req_s.send(Request::GrabJobUniq).await?;
								} else {
									debug!("[{}] got a noop, too busy, ignoring", self.name);
								}
							} else {
								trace!(
									"[{}] ignoring irrelevant packet {} ({})",
									self.name,
									pkt.name(),
									pkt.id()
								);
							}

							continue 'parse;
						}
						Err(DekuError::Parse(msg)) => {
							if msg.contains("not enough data") {
								debug!("[{}] got partial packet, waiting for more", self.name);
								continue 'recv;
							} else {
								warn!(
									"[{}] bad packet, throwing away {} bytes",
									self.name,
									packet.len()
								);
								warn!("[{}] parsing error: {}", self.name, msg);
								continue 'recv;
							}
						}
						Err(err) => {
							warn!(
								"[{}] bad packet, throwing away {} bytes",
								self.name,
								packet.len()
							);
							warn!("[{}] parsing error: {}", self.name, err);
							continue 'recv;
						}
					}
				}
			}
		})
	}
}