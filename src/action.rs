use serde::{Deserialize, Serialize};
use strum::Display;

#[derive(Debug, Clone, PartialEq, Eq, Display, Serialize, Deserialize)]
pub enum Action {
    Tick,
    Render,
    Resize(u16, u16),
    Suspend,
    Resume,
    Quit,
    ClearScreen,
    Error(String),
    Help,
    XStatus(bool),
    XTx {
        ok: bool,
        text: String,
    },
    /// Scanner started firing (first keystroke of a scan burst)
    SandStart,
    /// Complete payload decoded (Enter received)
    Scan(String),
    /// Radio connection state changed
    MeshStatus(bool),
    /// Text message received from the mesh
    MeshRx {
        from: String,
        text: String,
    },
    /// Result of an attempted mesh send
    MeshTx {
        ok: bool,
        text: String,
    },
}
