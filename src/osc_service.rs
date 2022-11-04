//! MIDI/OSC translator for Behringer BCR2000
//!
//! An OSC server receives OSC packets at a configured UDP port and translates
//! them to MIDI/BCL messages sent to a BCR2000.
//!
//! An OSC client listens for MIDI/BCL messages from a BCR2000, translates them
//! to OSC packets, and sends them to one or more configured UDP destinations.

use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;

use crate::midi_io::{MidiSink, MidiStream};
use crate::translator::{midi_msg_to_osc, osc_pkt_to_midi};
use crate::PGM;
use futures::{pin_mut, select, FutureExt, Sink, SinkExt, Stream, StreamExt};
use log::{debug, error, info};
use midi_control::MidiMessage;
use rosc::encoder::encode;
use tokio::net::UdpSocket;
use tokio::spawn;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

/// Data type used to distribute stop notifications to the various tasks started
/// by this module. Since there are a variety of ways to do this, it was
/// convenient to abstract this while experimenting.
type StopMechanism = Arc<Notify>;

/// Represents the OSC client/server. The start method starts listeners for OSC
/// and MIDI traffic. The stop method shuts everything down.
///
/// You should call stop before dropping this object. Otherwise the I/O tasks
/// will continue running, with no way to stop them.
///
pub struct BCtlOscSvc {
    pub midi_in_port_name: String,
    pub midi_out_port_name: String,
    pub osc_in_addr: SocketAddr,
    pub osc_out_addrs: Arc<Vec<SocketAddr>>,

    stopper: StopMechanism,
    spawned_tasks: Vec<JoinHandle<()>>,
}
impl BCtlOscSvc {
    /// Create a new B-Control OSC service object.
    ///
    /// The MIDI input and output ports should be chosen such that MIDI commands
    /// will reach your B-Control devices, and replies from the controllers will
    /// make it back to this service.
    ///
    pub fn new(
        midi_in_port_name: &str,
        midi_out_port_name: &str,
        osc_in_addr: &SocketAddr,
        osc_out_addrs: &[SocketAddr],
    ) -> Self {
        BCtlOscSvc {
            midi_in_port_name: midi_in_port_name.to_string(),
            midi_out_port_name: midi_out_port_name.to_string(),
            osc_in_addr: osc_in_addr.clone(),
            osc_out_addrs: Arc::new(osc_out_addrs.to_vec()),
            stopper: Arc::new(Notify::new()),
            spawned_tasks: Vec::new(),
        }
    }

    /// Start the I/O tasks that listen for and respond to OSC and MIDI
    /// messages.
    pub async fn start(&mut self) -> Result<(), Box<dyn Error>> {
        // We use a single UDP socket for sending and receiving.
        let udp_socket = Arc::new(UdpSocket::bind(self.osc_in_addr).await?);

        // MIDI -> OSC
        let midi_rx = MidiStream::bind(&self.midi_in_port_name)?;
        info!(
            "{PGM} is listening for MIDI on \"{}\"",
            self.midi_in_port_name
        );
        self.spawned_tasks
            .push(self.start_midi_to_osc(midi_rx, &udp_socket));

        // OSC -> MIDI
        let midi_tx = MidiSink::bind(&self.midi_out_port_name)?;
        info!("{PGM} will send MIDI to \"{}\".", self.midi_out_port_name);
        self.spawned_tasks
            .push(self.start_osc_to_midi(&udp_socket, midi_tx));

        Ok(())
    }

    /// Stop the I/O tasks started by start(). Returns after all tasks have
    /// terminated.
    pub async fn stop(&mut self) {
        self.stopper.notify_waiters();
        for ele in self.spawned_tasks.drain(0..) {
            // Seems to pend if the task completed. Might have to live with
            // .cancel instead if this becomes a problem.
            ele.await
                .or_else(|e| {
                    error!("Error waiting for tasks to stop: {e}");
                    Err(e)
                })
                .ok();
        }
    }

    fn start_midi_to_osc(
        &self,
        receiver: impl Stream<Item = MidiMessage> + Send + 'static,
        udp_socket: &Arc<UdpSocket>,
    ) -> JoinHandle<()> {
        let stopper = self.stopper.clone();
        let future = run_midi_to_osc(
            stopper,
            receiver,
            self.osc_out_addrs.clone(),
            udp_socket.clone(),
        );
        spawn(future)
    }

    fn start_osc_to_midi(
        &self,
        udp_socket: &Arc<UdpSocket>,
        dest: impl Sink<MidiMessage> + Send + 'static,
    ) -> JoinHandle<()> {
        let stopper = self.stopper.clone();
        let udp_socket = udp_socket.clone();
        let future = run_osc_to_midi(stopper, udp_socket, dest);
        spawn(future)
    }
}

async fn wait_on_stopping(stopper: StopMechanism) {
    stopper.notified().await;
}

async fn run_midi_to_osc<SRC>(
    stopper: StopMechanism,
    src: SRC,
    osc_out_addrs: Arc<Vec<SocketAddr>>,
    dest: Arc<UdpSocket>,
) where
    SRC: Stream<Item = MidiMessage> + Send,
{
    let stopper = stopper.clone();
    select! {
        _ = run_midi_to_osc_loop(src, osc_out_addrs, dest).fuse() => {},
        _ = wait_on_stopping(stopper).fuse() => {}
    };
    info!("{PGM} OSC listener stopped.");
}

async fn run_midi_to_osc_loop<SRC>(
    src: SRC,
    osc_out_addrs: Arc<Vec<SocketAddr>>,
    dest: Arc<UdpSocket>,
) where
    SRC: Stream<Item = MidiMessage> + Send,
{
    pin_mut!(src);
    info!("{PGM} will send OSC from UDP port {:?}.", dest.local_addr());
    while let Some(midi_msg) = src.next().await {
        if let Some(pkt) = midi_msg_to_osc(midi_msg) {
            let e = encode(&pkt);
            match e {
                Ok(buf) => {
                    debug!("Sending this OSC packet: {pkt:?}");
                    for a in &*osc_out_addrs {
                        if let Err(e) = dest.send_to(&buf, a).await {
                            error!("OSC send to {a} failed: {e}");
                        };
                    }
                }
                Err(e) => error!("OSC encoding failed: {e}"),
            }
        }
    }
    info!("{PGM} OSC sender stopped.");
}

async fn run_osc_to_midi<D>(stopper: StopMechanism, src: Arc<UdpSocket>, dest: D)
where
    D: Sink<MidiMessage>,
{
    let stopper = stopper.clone();
    select! {
        _ = run_osc_to_midi_loop(src, dest).fuse() => {},
        _ = wait_on_stopping(stopper).fuse() => {}
    };
    info!("{PGM} OSC listener stopped.");
}

async fn run_osc_to_midi_loop<D>(src: Arc<UdpSocket>, dest: D)
where
    D: Sink<MidiMessage>,
{
    info!(
        "{PGM} listening for OSC on UDP port {:?}.",
        src.local_addr()
    );
    let mut vec = vec![0u8; 1024 * 16];
    let mut next: usize = 0;
    pin_mut!(dest);
    loop {
        // TODO: On Windows, we get error 10054 here if the *sender* just tried
        // to send to an unresponsive port! (Try using distinct send/receive
        // UdpSockets?)
        match src.recv_from(&mut vec[next..]).await {
            Ok((len, sender)) => {
                let buflen = next + len;
                match rosc::decoder::decode_udp(&vec[0..buflen]) {
                    Ok((remainder, pkt)) => {
                        debug!("Received OSC packet from {sender:?}: {pkt:?}");
                        let rlen = remainder.len();
                        if rlen > 0 {
                            debug!("OSC input remainder {len} bytes.");
                            vec.copy_within(len..len + rlen, 0);
                            next = rlen;
                        }
                        for m in osc_pkt_to_midi(&pkt) {
                            dest.feed(m)
                                .await
                                .unwrap_or_else(|_| error!("OSC pkt feed failed."));
                        }
                        dest.flush()
                            .await
                            .unwrap_or_else(|_| error!("OSC pkt flush failed."));
                    }
                    Err(e) => {
                        error!("OSC pkt decode error: {e}");
                        next = 0;
                        error!("Discarded {buflen} bytes.");
                    }
                }
            }
            Err(e) => error!("UDP recv error: {e}"),
        }
    }
}
