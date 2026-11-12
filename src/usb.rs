//! USB transport: claims interface 0 (HID) of the receiver over usbfs with `nusb`.
//!
//! The Linux kernel drops the receiver's replies on hidraw (they use the undeclared
//! report ID 0xBB), so the app talks to the interrupt endpoints directly. Only
//! interface 0 is detached; the audio interfaces stay with `snd-usb-audio`, so the
//! mic keeps working while the app runs. Dropping the transport hands interface 0
//! back to `usbhid`.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use nusb::MaybeFuture;
use nusb::descriptors::TransferType;
use nusb::transfer::{Buffer, Direction, In, Interrupt, Out};

use crate::protocol::{self, Model, PACKET_LEN, Reply, Unit};

/// Why the receiver could not be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    NotFound,
    /// The USB device node isn't writable: the udev rule is missing.
    Permission(String),
    /// Another program holds interface 0 (for example the web UI's bridge).
    Busy(String),
    Other(String),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::NotFound => write!(f, "no Lark M2 receiver found"),
            OpenError::Permission(e) => write!(f, "no permission to open the receiver: {e}"),
            OpenError::Busy(e) => write!(f, "the receiver is in use by another program: {e}"),
            OpenError::Other(e) => write!(f, "{e}"),
        }
    }
}

/// Talks to one receiver. Implemented by [`UsbTransport`] and the demo simulator.
pub trait Transport: Send {
    fn model(&self) -> Model;
    /// Send one request and wait for the reply with the same command byte.
    fn request(&mut self, command: u8, unit: Unit, payload: &[u8], timeout: Duration) -> Result<Option<Reply>>;
}

pub struct UsbTransport {
    model: Model,
    _interface: nusb::Interface,
    ep_out: nusb::Endpoint<Interrupt, Out>,
    ep_in: nusb::Endpoint<Interrupt, In>,
}

/// Find a supported receiver.
pub fn find() -> Result<Option<(nusb::DeviceInfo, Model)>> {
    let devices = nusb::list_devices().wait().context("listing USB devices")?;
    for d in devices {
        if let Some(m) = protocol::model_for(d.vendor_id(), d.product_id()) {
            return Ok(Some((d, m)));
        }
    }
    Ok(None)
}

/// Is a supported receiver plugged in (without opening it)?
pub fn present() -> bool {
    matches!(find(), Ok(Some(_)))
}

impl UsbTransport {
    pub fn open() -> std::result::Result<UsbTransport, OpenError> {
        let (info, model) = match find() {
            Ok(Some(x)) => x,
            Ok(None) => return Err(OpenError::NotFound),
            Err(e) => return Err(OpenError::Other(e.to_string())),
        };
        let device = info.open().wait().map_err(classify)?;
        let interface = device.detach_and_claim_interface(0).wait().map_err(classify)?;

        // Endpoint addresses from the descriptor (0x03 OUT / 0x82 IN on the M2).
        let (mut out_addr, mut in_addr) = (None, None);
        if let Some(desc) = interface.descriptor() {
            for ep in desc.endpoints() {
                if ep.transfer_type() != TransferType::Interrupt {
                    continue;
                }
                match ep.direction() {
                    Direction::Out => out_addr = out_addr.or(Some(ep.address())),
                    Direction::In => in_addr = in_addr.or(Some(ep.address())),
                }
            }
        }
        let out_addr = out_addr.unwrap_or(0x03);
        let in_addr = in_addr.unwrap_or(0x82);
        let ep_out = interface
            .endpoint::<Interrupt, Out>(out_addr)
            .map_err(|e| OpenError::Other(format!("OUT endpoint: {e}")))?;
        let mut ep_in =
            interface.endpoint::<Interrupt, In>(in_addr).map_err(|e| OpenError::Other(format!("IN endpoint: {e}")))?;
        for _ in 0..4 {
            let b = ep_in.allocate(PACKET_LEN);
            ep_in.submit(b);
        }
        log::info!(
            "opened {} ({:04x}:{:04x}), IN {in_addr:#04x} OUT {out_addr:#04x}",
            model.name,
            model.vid,
            model.pid
        );
        Ok(UsbTransport { model, _interface: interface, ep_out, ep_in })
    }

    /// Collect IN reports until `deadline`; return the first reply for `command`.
    fn read_reply(&mut self, command: u8, deadline: Instant) -> Result<Option<Reply>> {
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            let Some(c) = self.ep_in.wait_next_complete(deadline - now) else {
                return Ok(None);
            };
            // A failed transfer ends the session (the worker reconnects). Resubmitting it
            // would fail again at once, and the loop would spin.
            if let Err(e) = c.status {
                return Err(anyhow!("USB read failed: {e}"));
            }
            let data = c.buffer[..c.actual_len.min(c.buffer.len())].to_vec();
            let mut buf = c.buffer;
            buf.clear();
            buf.set_requested_len(PACKET_LEN);
            self.ep_in.submit(buf);
            match protocol::decode(&data) {
                Some(r) if r.command == command => return Ok(Some(r)),
                Some(r) => log::debug!("ignored reply {:#04x} from {:?}", r.command, r.unit),
                None => log::trace!("ignored IN report {}", hex::encode(&data)),
            }
        }
    }

    /// Throw away replies left over from earlier requests (each request gets two IN reports).
    /// Bounded, and a failed transfer is an error, so this can never spin.
    fn drain(&mut self) -> Result<()> {
        for _ in 0..16 {
            let Some(c) = self.ep_in.wait_next_complete(Duration::ZERO) else { return Ok(()) };
            if let Err(e) = c.status {
                return Err(anyhow!("USB read failed: {e}"));
            }
            let mut buf = c.buffer;
            buf.clear();
            buf.set_requested_len(PACKET_LEN);
            self.ep_in.submit(buf);
        }
        Err(anyhow!("the receiver keeps sending data"))
    }
}

impl Transport for UsbTransport {
    fn model(&self) -> Model {
        self.model
    }

    fn request(&mut self, command: u8, unit: Unit, payload: &[u8], timeout: Duration) -> Result<Option<Reply>> {
        self.drain()?;
        let packet = protocol::encode(command, unit, payload);
        log::trace!("TX {}", hex::encode(&packet[..6 + payload.len()]));
        self.ep_out.submit(Buffer::from(packet.to_vec()));
        let done =
            self.ep_out.wait_next_complete(Duration::from_millis(500)).ok_or_else(|| anyhow!("USB write timed out"))?;
        done.status.map_err(|e| anyhow!("USB write failed: {e}"))?;
        let reply = self.read_reply(command, Instant::now() + timeout)?;
        if let Some(r) = &reply {
            log::trace!("RX {:#04x} {:?} {}", r.command, r.unit, hex::encode(&r.payload));
        }
        // The apps leave ~20 ms between frames.
        std::thread::sleep(Duration::from_millis(20));
        Ok(reply)
    }
}

fn classify(e: nusb::Error) -> OpenError {
    let msg = e.to_string();
    match e.kind() {
        nusb::ErrorKind::PermissionDenied => OpenError::Permission(msg),
        nusb::ErrorKind::Busy => OpenError::Busy(msg),
        nusb::ErrorKind::NotFound | nusb::ErrorKind::Disconnected => OpenError::NotFound,
        _ => OpenError::Other(msg),
    }
}
