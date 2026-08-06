use std::time::Duration;

use meshtastic::{
    api::StreamApi,
    packet::{PacketDestination, PacketRouter},
    protobufs::{FromRadio, MeshPacket, PortNum, from_radio, mesh_packet},
    types::NodeId,
    utils,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::{error, info, warn};

use crate::action::Action;

/// 0 = DEFCONnect, 1 = HackerComms, 2 = NodeChat
const MESH_CHANNEL: u32 = 0;
const SERIAL_PORT: &str = "/dev/ttyACM0";
/// LoRa text payloads top out ~230 bytes; stay well under
const MAX_PAYLOAD_BYTES: usize = 200;

/// Router required by send_text (used for packet echo bookkeeping).
struct ScanRouter {
    my_id: NodeId,
}

impl PacketRouter<(), std::io::Error> for ScanRouter {
    fn handle_packet_from_radio(&mut self, _packet: FromRadio) -> Result<(), std::io::Error> {
        Ok(())
    }
    fn handle_mesh_packet(&mut self, _packet: MeshPacket) -> Result<(), std::io::Error> {
        Ok(())
    }
    fn source_node_id(&self) -> NodeId {
        self.my_id
    }
}

fn truncate_payload(s: &str) -> String {
    if s.len() <= MAX_PAYLOAD_BYTES {
        return s.to_string();
    }
    let mut end = MAX_PAYLOAD_BYTES;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// Spawn the mesh worker task. `outbound` receives scan payloads to broadcast.
/// Reconnects forever if the radio is missing or drops.
pub fn spawn(action_tx: UnboundedSender<Action>, mut outbound: UnboundedReceiver<String>) {
    tokio::spawn(async move {
        loop {
            let _ = action_tx.send(Action::MeshStatus(false));
            match run_radio(&action_tx, &mut outbound).await {
                Ok(()) => warn!("radio connection closed"),
                Err(e) => warn!("radio error: {e}"),
            }
            let _ = action_tx.send(Action::MeshStatus(false));
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}

async fn run_radio(
    action_tx: &UnboundedSender<Action>,
    outbound: &mut UnboundedReceiver<String>,
) -> color_eyre::Result<()> {
    let serial = utils::stream::build_serial_stream(SERIAL_PORT.to_string(), None, None, None)
        .map_err(|e| color_eyre::eyre::eyre!("open {SERIAL_PORT}: {e}"))?;
    let (mut decoded_listener, stream_api) = StreamApi::new().connect(serial).await;
    let mut stream_api = stream_api.configure(utils::generate_rand_id()).await?;
    info!("meshtastic radio configured on {SERIAL_PORT}");
    let _ = action_tx.send(Action::MeshStatus(true));

    let mut router = ScanRouter {
        my_id: NodeId::new(0),
    };

    loop {
        tokio::select! {
            // Outbound: scan payloads from the TUI
            Some(payload) = outbound.recv() => {
                let text = truncate_payload(&payload);
                match stream_api
                    .send_text(&mut router, text.clone(), PacketDestination::Broadcast, true, MESH_CHANNEL.into())
                    .await
                {
                    Ok(()) => { let _ = action_tx.send(Action::MeshTx { ok: true, text }); }
                    Err(e) => {
                        error!("mesh send failed: {e}");
                        let _ = action_tx.send(Action::MeshTx { ok: false, text });
                    }
                }
            }

            // Inbound: packets from the radio
            maybe_packet = decoded_listener.recv() => {
                let Some(packet) = maybe_packet else {
                    return Ok(()); // radio disconnected
                };
                match packet.payload_variant {
                    Some(from_radio::PayloadVariant::MyInfo(info)) => {
                        router.my_id = NodeId::new(info.my_node_num);
                        info!("own node id: !{:08x}", info.my_node_num);
                    }
                    Some(from_radio::PayloadVariant::Packet(mp)) => {
                        if let Some(mesh_packet::PayloadVariant::Decoded(data)) = mp.payload_variant {
                            if data.portnum == PortNum::TextMessageApp as i32 {
                                let text = String::from_utf8_lossy(&data.payload).to_string();
                                let from = format!("!{:08x}", mp.from);
                                let _ = action_tx.send(Action::MeshRx { from, text });
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}
