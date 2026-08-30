//! Wire protocol for Niri's IPC socket.
//!
//! Niri exports the socket path in `$NIRI_SOCKET` for every process it spawns.
//! The protocol is deliberately trivial: connect, write one JSON request on a
//! single line, shut down the write half, read one JSON line back. The reply
//! is `{"Ok": <response>}` or `{"Err": "<message>"}`.
//!
//! We speak the protocol directly rather than depending on the `niri-ipc`
//! crate or shelling out to `niri msg --json`. `niri-ipc` is explicitly *not*
//! semver-stable — it tracks the compositor's own version, so pinning it would
//! couple this crate's `Cargo.lock` to whichever Niri the user happens to be
//! running. The JSON *is* documented as stable (existing fields and variants
//! are not renamed or removed; new ones may be added). Shelling out to
//! `niri msg` would fork a process per call and parse output the Niri docs
//! explicitly decline to keep stable in its non-JSON form. Hand-written
//! structs against the stable JSON is the narrowest coupling of the three.
//!
//! Every deserialized struct here declares only the fields LoFi reads, and
//! `serde` ignores the rest — which is exactly the "handle new fields
//! gracefully" the Niri IPC documentation asks of clients. Optional fields are
//! `Option` + `#[serde(default)]` so a *missing* field degrades instead of
//! failing the whole parse.

use std::env;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Cap on how long a single request may block the launcher.
///
/// The socket is local and every request we make is answered in well under a
/// millisecond, so this is not a latency budget — it is a liveness guard. A
/// compositor wedged mid-frame must not wedge the launcher with it; LoFi's
/// whole premise is that it appears instantly, and an unbounded blocking read
/// on the GTK main thread would trade that for a frozen empty window.
const TIMEOUT: Duration = Duration::from_millis(1000);

/// A request to the compositor. Serde's default externally-tagged
/// representation is exactly Niri's wire format: unit variants become bare
/// strings (`"Windows"`), newtype variants become one-key objects
/// (`{"Action":{…}}`).
#[derive(Debug, Serialize)]
pub enum Request {
    /// Every open window, in unspecified order (see `NiriWindow::sort_key`).
    Windows,
    /// Every workspace on every output.
    Workspaces,
    /// Every connected output, keyed by connector name.
    Outputs,
    /// Perform an action.
    Action(Action),
}

/// The subset of Niri's action vocabulary LoFi dispatches.
///
/// Every variant that targets a window carries an explicit `id`, with the
/// single documented exception of `ExpandColumnToAvailableWidth`. That is a
/// hard requirement, not a preference: by the time the user presses Enter the
/// launcher has been on screen, and an action phrased against "the focused
/// window" could land on the wrong one. Niri's other focused-window-only
/// actions (`MaximizeColumn`, `SetColumnWidth`, …) are therefore deliberately
/// absent — each has a by-id equivalent we use instead (see
/// `super::run_command`).
///
/// Field names and variant names are the compositor's, not ours; they must
/// match the `niri-ipc` `Action` enum exactly. The canonical way to confirm a
/// shape is to point `NIRI_SOCKET` at a listening socket and run the
/// equivalent `niri msg action …`, which prints the JSON it would have sent.
#[derive(Debug, Serialize)]
pub enum Action {
    /// Raise the window, switching workspace and output if needed.
    FocusWindow { id: u64 },
    /// Switch to a workspace.
    FocusWorkspace { reference: WorkspaceReference },
    /// Move a window to a workspace. `focus: true` makes the focus follow it,
    /// which is the same "follow the window you just moved" behaviour the
    /// GNOME backend gets from its move-then-switch pair.
    MoveWindowToWorkspace {
        window_id: u64,
        reference: WorkspaceReference,
        focus: bool,
    },
    /// Toggle fullscreen.
    FullscreenWindow { id: u64 },
    /// Toggle maximized-to-edges — Niri's equivalent of the maximize other
    /// desktops put on the titlebar.
    MaximizeWindowToEdges { id: u64 },
    /// Centre the window's column in the view.
    CenterWindow { id: u64 },
    /// Move the window between the tiling and floating layouts.
    ToggleWindowFloating { id: u64 },
    /// Set the window's width. For a window in the scrolling layout this is
    /// its column's width.
    SetWindowWidth { id: u64, change: SizeChange },
    /// Widen the active column to fill the space no other fully-visible
    /// column is using.
    ///
    /// The one action here with no by-id form. It acts on Niri's *layout*
    /// active window, which is not the same thing as the window that reports
    /// `is_focused`: while LoFi's layer-shell overlay holds exclusive keyboard
    /// focus no window reports `is_focused` at all, yet the action still lands
    /// on the column the user was last working in. That is also the window
    /// LoFi picks as its command target, because switching to a non-empty
    /// workspace focuses a window there and so bumps its `focus_timestamp`.
    /// The two can only diverge when the active workspace is empty — in which
    /// case there is no column and the action is a harmless no-op.
    ExpandColumnToAvailableWidth {},
    /// Close the window.
    CloseWindow { id: u64 },
    /// Exit the compositor — Niri's "log out".
    Quit { skip_confirmation: bool },
}

/// How Niri identifies a workspace in an action.
///
/// LoFi always uses `Id`. The alternative, `Index`, is Niri's *per-output*
/// 1-based position, so on a multi-monitor session index 2 is ambiguous
/// between monitors. Ids are globally unique, and they are what the
/// `Workspaces` response hands us in the first place.
#[derive(Debug, Serialize)]
pub enum WorkspaceReference {
    Id(u64),
}

/// A width/height change. LoFi only ever uses the proportional form, which is
/// a percentage of the working area.
#[derive(Debug, Serialize)]
pub enum SizeChange {
    SetProportion(f64),
}

/// Reply envelope. Externally tagged, matching `{"Ok":…}` / `{"Err":"…"}`.
#[derive(Debug, Deserialize)]
enum Reply<T> {
    Ok(T),
    Err(String),
}

/// The responses LoFi asks for. `Handled` is the reply to every `Action`.
///
/// This enum is not exhaustive over Niri's response vocabulary — it covers the
/// four requests above. A response we didn't ask for would fail to deserialize
/// and surface as `NiriError::Json`, which is the correct outcome: it would
/// mean the request/response pairing had gone wrong.
#[derive(Debug, Deserialize)]
pub enum Response {
    Handled,
    Windows(Vec<NiriWindow>),
    Workspaces(Vec<NiriWorkspace>),
    /// Keyed by connector name (`"DP-3"`), which is also `NiriOutput::name`
    /// and the value `NiriWorkspace::output` carries.
    Outputs(std::collections::HashMap<String, NiriOutput>),
}

/// An open window as Niri reports it.
#[derive(Debug, Deserialize)]
pub struct NiriWindow {
    pub id: u64,
    #[serde(default)]
    pub title: Option<String>,
    /// The Wayland `app_id` (or the XWayland WM_CLASS). This is *not* a
    /// `.desktop` id — resolving one to the other is `super::appid`'s job.
    #[serde(default)]
    pub app_id: Option<String>,
    /// `None` for a window not currently assigned to a workspace.
    #[serde(default)]
    pub workspace_id: Option<u64>,
    #[serde(default)]
    pub is_focused: bool,
    /// When the window last received focus, as a monotonic duration. Niri
    /// does not return windows in MRU order, so this is what LoFi sorts on.
    /// `None` for a window that has never been focused, which sorts last.
    #[serde(default)]
    pub focus_timestamp: Option<NiriDuration>,
}

impl NiriWindow {
    /// Descending-MRU sort key: most recently focused first.
    ///
    /// `is_focused` is folded in ahead of the timestamp because Niri commits a
    /// window to its recent-windows list on a debounce (750ms by default), so
    /// the currently-focused window's `focus_timestamp` can still be the
    /// *previous* window's. Without the tiebreak, opening the launcher within
    /// that window would target the wrong window.
    ///
    /// A window that has never been focused (`None`) sorts to the very end
    /// rather than the front, so a freshly-mapped window doesn't hijack the
    /// command set's target.
    pub fn sort_key(&self) -> (bool, u64, u32) {
        let (secs, nanos) = match &self.focus_timestamp {
            Some(d) => (d.secs, d.nanos),
            None => (0, 0),
        };
        (self.is_focused, secs, nanos)
    }
}

/// `std::time::Duration` as serde renders it. Declared explicitly rather than
/// deserializing into `Duration` so that a future Niri that adds fields here,
/// or renders the value differently, degrades to `None` at the `Option` above
/// instead of aborting the whole window list.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct NiriDuration {
    pub secs: u64,
    pub nanos: u32,
}

/// A workspace as Niri reports it.
#[derive(Debug, Deserialize)]
pub struct NiriWorkspace {
    /// Globally unique and stable *while the workspace exists*. Niri's
    /// dynamic workspaces mean the trailing empty workspace on each output is
    /// destroyed and recreated with a fresh id as you move away from it, so
    /// this is not a durable identifier across launcher invocations — which
    /// is why LoFi's persistent MRU keys on position, not on this. See
    /// `super::NiriBackend::workspace_table`.
    pub id: u64,
    /// 1-based position of this workspace on its own output.
    #[serde(default)]
    pub idx: u32,
    /// Set only for a named workspace; `None` for the numbered default.
    #[serde(default)]
    pub name: Option<String>,
    /// Connector name of the output this workspace lives on.
    #[serde(default)]
    pub output: Option<String>,
}

/// A connected output.
#[derive(Debug, Deserialize)]
pub struct NiriOutput {
    pub name: String,
    /// `None` when the output is disabled — it has no position or size then.
    #[serde(default)]
    pub logical: Option<NiriLogicalOutput>,
}

/// An output's placement in the global logical coordinate space, in the same
/// logical pixels window geometry is reported in.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct NiriLogicalOutput {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Everything that can go wrong talking to the compositor.
#[derive(Debug)]
pub enum NiriError {
    /// `$NIRI_SOCKET` is unset or empty — Niri isn't running, or the launcher
    /// was started from an environment that didn't inherit it.
    NoSocket,
    Io(std::io::Error),
    Json(serde_json::Error),
    /// The compositor answered `{"Err": …}`.
    Compositor(String),
    /// The compositor answered `Ok`, but with a response of the wrong shape
    /// for the request we sent.
    UnexpectedResponse(&'static str),
}

impl std::fmt::Display for NiriError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NiriError::NoSocket => write!(f, "NIRI_SOCKET is unset or empty"),
            NiriError::Io(e) => write!(f, "socket error: {e}"),
            NiriError::Json(e) => write!(f, "protocol error: {e}"),
            NiriError::Compositor(m) => write!(f, "compositor returned an error: {m}"),
            NiriError::UnexpectedResponse(what) => {
                write!(
                    f,
                    "compositor returned an unexpected response (wanted {what})"
                )
            }
        }
    }
}

impl From<std::io::Error> for NiriError {
    fn from(e: std::io::Error) -> Self {
        NiriError::Io(e)
    }
}

impl From<serde_json::Error> for NiriError {
    fn from(e: serde_json::Error) -> Self {
        NiriError::Json(e)
    }
}

/// Send one request and read one reply.
///
/// A fresh connection per call, for the same reason the GNOME backend opens a
/// fresh D-Bus connection per call: the launcher process lives for a few
/// seconds and makes a handful of requests, so pooling would buy nothing and
/// cost a lifetime to manage. Shutting down the write half is what tells Niri
/// the request is complete — the trailing newline alone also works, but doing
/// both matches what the documented `socat` recipe does and costs nothing.
pub fn request(request: &Request) -> Result<Response, NiriError> {
    let path = match env::var("NIRI_SOCKET") {
        Ok(p) if !p.is_empty() => p,
        _ => return Err(NiriError::NoSocket),
    };

    let stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;

    let mut writer = &stream;
    let payload = serde_json::to_string(request)?;
    writer.write_all(payload.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    stream.shutdown(std::net::Shutdown::Write)?;

    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line)?;

    match serde_json::from_str::<Reply<Response>>(&line)? {
        Reply::Ok(response) => Ok(response),
        Reply::Err(message) => Err(NiriError::Compositor(message)),
    }
}

/// `Request::Windows`, unwrapped to the window list.
pub fn windows() -> Result<Vec<NiriWindow>, NiriError> {
    match request(&Request::Windows)? {
        Response::Windows(w) => Ok(w),
        _ => Err(NiriError::UnexpectedResponse("Windows")),
    }
}

/// `Request::Workspaces`, unwrapped to the workspace list.
pub fn workspaces() -> Result<Vec<NiriWorkspace>, NiriError> {
    match request(&Request::Workspaces)? {
        Response::Workspaces(w) => Ok(w),
        _ => Err(NiriError::UnexpectedResponse("Workspaces")),
    }
}

/// `Request::Outputs`, unwrapped to the connector-name-keyed output map.
pub fn outputs() -> Result<std::collections::HashMap<String, NiriOutput>, NiriError> {
    match request(&Request::Outputs)? {
        Response::Outputs(o) => Ok(o),
        _ => Err(NiriError::UnexpectedResponse("Outputs")),
    }
}

/// Perform an action. The compositor answers `Handled` for any action it
/// accepted — including ones that turned out to be no-ops, e.g. an id that no
/// longer resolves. There is nothing useful to do about that from here (the
/// launcher window has already closed), which is why the caller logs rather
/// than retrying.
pub fn action(action: Action) -> Result<(), NiriError> {
    match request(&Request::Action(action))? {
        Response::Handled => Ok(()),
        _ => Err(NiriError::UnexpectedResponse("Handled")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The request wire format is the whole contract with the compositor, and
    /// it is the one thing a unit test can pin without a running Niri. Each
    /// expected string below was captured from the real `niri msg` client by
    /// pointing `NIRI_SOCKET` at a listening socket and reading what it sent.
    #[test]
    fn requests_serialize_to_niris_wire_format() {
        let cases: Vec<(Request, &str)> = vec![
            (Request::Windows, r#""Windows""#),
            (Request::Workspaces, r#""Workspaces""#),
            (Request::Outputs, r#""Outputs""#),
            (
                Request::Action(Action::FocusWindow { id: 3 }),
                r#"{"Action":{"FocusWindow":{"id":3}}}"#,
            ),
            (
                Request::Action(Action::FocusWorkspace {
                    reference: WorkspaceReference::Id(7),
                }),
                r#"{"Action":{"FocusWorkspace":{"reference":{"Id":7}}}}"#,
            ),
            (
                Request::Action(Action::MoveWindowToWorkspace {
                    window_id: 3,
                    reference: WorkspaceReference::Id(7),
                    focus: true,
                }),
                r#"{"Action":{"MoveWindowToWorkspace":{"window_id":3,"reference":{"Id":7},"focus":true}}}"#,
            ),
            (
                Request::Action(Action::FullscreenWindow { id: 3 }),
                r#"{"Action":{"FullscreenWindow":{"id":3}}}"#,
            ),
            (
                Request::Action(Action::MaximizeWindowToEdges { id: 3 }),
                r#"{"Action":{"MaximizeWindowToEdges":{"id":3}}}"#,
            ),
            (
                Request::Action(Action::CenterWindow { id: 3 }),
                r#"{"Action":{"CenterWindow":{"id":3}}}"#,
            ),
            (
                Request::Action(Action::ToggleWindowFloating { id: 3 }),
                r#"{"Action":{"ToggleWindowFloating":{"id":3}}}"#,
            ),
            (
                Request::Action(Action::SetWindowWidth {
                    id: 3,
                    change: SizeChange::SetProportion(50.0),
                }),
                r#"{"Action":{"SetWindowWidth":{"id":3,"change":{"SetProportion":50.0}}}}"#,
            ),
            (
                Request::Action(Action::ExpandColumnToAvailableWidth {}),
                r#"{"Action":{"ExpandColumnToAvailableWidth":{}}}"#,
            ),
            (
                Request::Action(Action::CloseWindow { id: 3 }),
                r#"{"Action":{"CloseWindow":{"id":3}}}"#,
            ),
            (
                Request::Action(Action::Quit {
                    skip_confirmation: false,
                }),
                r#"{"Action":{"Quit":{"skip_confirmation":false}}}"#,
            ),
        ];

        for (request, expected) in cases {
            let actual = serde_json::to_string(&request).expect("request should serialize");
            assert_eq!(
                actual, expected,
                "{request:?} must serialize to Niri's wire format; got {actual}"
            );
        }
    }

    #[test]
    fn replies_deserialize_from_niris_wire_format() {
        // Captured from a live compositor. Only the fields LoFi reads are
        // asserted; the rest exercise serde's ignore-unknown behaviour, which
        // is what keeps this client working as Niri adds fields.
        let raw = r#"{"Ok":{"Windows":[{"id":2,"title":"main","app_id":"com.mitchellh.ghostty","pid":4534,"workspace_id":1,"is_focused":false,"is_floating":false,"is_urgent":false,"layout":{"pos_in_scrolling_layout":[1,1],"tile_size":[3048.0,1672.8],"window_size":[3048,1673],"tile_pos_in_workspace_view":null,"window_offset_in_tile":[0.0,0.0]},"focus_timestamp":{"secs":769,"nanos":594468267}}]}}"#;
        let reply: Reply<Response> =
            serde_json::from_str(raw).expect("a real Windows reply should deserialize");
        let Reply::Ok(Response::Windows(windows)) = reply else {
            panic!("expected Ok(Windows), got {reply:?}");
        };
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].id, 2);
        assert_eq!(windows[0].title.as_deref(), Some("main"));
        assert_eq!(windows[0].app_id.as_deref(), Some("com.mitchellh.ghostty"));
        assert_eq!(windows[0].workspace_id, Some(1));
        assert_eq!(windows[0].sort_key(), (false, 769, 594468267));

        let raw = r#"{"Ok":{"Workspaces":[{"id":2,"idx":2,"name":null,"output":"DP-3","is_urgent":false,"is_active":false,"is_focused":false,"active_window_id":null}]}}"#;
        let reply: Reply<Response> =
            serde_json::from_str(raw).expect("a real Workspaces reply should deserialize");
        let Reply::Ok(Response::Workspaces(workspaces)) = reply else {
            panic!("expected Ok(Workspaces), got {reply:?}");
        };
        assert_eq!(workspaces[0].id, 2);
        assert_eq!(workspaces[0].idx, 2);
        assert_eq!(workspaces[0].name, None);
        assert_eq!(workspaces[0].output.as_deref(), Some("DP-3"));

        let raw = r#"{"Ok":{"Outputs":{"DP-3":{"name":"DP-3","make":"LG","model":"HDR 4K","serial":"x","physical_size":[600,340],"modes":[],"current_mode":0,"is_custom_mode":false,"vrr_supported":false,"vrr_enabled":false,"logical":{"x":0,"y":0,"width":3072,"height":1728,"scale":1.25,"transform":"Normal"}}}}}"#;
        let reply: Reply<Response> =
            serde_json::from_str(raw).expect("a real Outputs reply should deserialize");
        let Reply::Ok(Response::Outputs(outputs)) = reply else {
            panic!("expected Ok(Outputs), got {reply:?}");
        };
        let output = outputs.get("DP-3").expect("DP-3 should be present");
        let logical = output.logical.expect("DP-3 should have a logical rect");
        assert_eq!(
            (logical.x, logical.y, logical.width, logical.height),
            (0, 0, 3072, 1728)
        );

        let reply: Reply<Response> = serde_json::from_str(r#"{"Ok":"Handled"}"#)
            .expect("an action reply should deserialize");
        assert!(
            matches!(reply, Reply::Ok(Response::Handled)),
            "an action reply should be Ok(Handled); got {reply:?}"
        );

        let reply: Reply<Response> = serde_json::from_str(r#"{"Err":"error parsing request"}"#)
            .expect("an error reply should deserialize");
        assert!(
            matches!(reply, Reply::Err(ref m) if m == "error parsing request"),
            "an error reply should carry the message; got {reply:?}"
        );
    }

    #[test]
    fn window_sort_key_orders_most_recently_focused_first() {
        fn window(id: u64, is_focused: bool, secs: u64, nanos: u32) -> NiriWindow {
            NiriWindow {
                id,
                title: None,
                app_id: None,
                workspace_id: None,
                is_focused,
                focus_timestamp: Some(NiriDuration { secs, nanos }),
            }
        }

        let mut windows = [
            window(1, false, 100, 0),
            window(2, false, 300, 0),
            window(3, false, 200, 500),
            window(4, false, 200, 0),
        ];
        windows.sort_by_key(|w| std::cmp::Reverse(w.sort_key()));
        assert_eq!(
            windows.iter().map(|w| w.id).collect::<Vec<_>>(),
            vec![2, 3, 4, 1],
            "windows should sort most-recently-focused first, with nanos breaking a secs tie"
        );

        // The live-focused window wins even when its committed timestamp is
        // older than another window's — Niri debounces the recent-windows
        // commit, so within that window the timestamp lags reality.
        let mut windows = [window(1, false, 900, 0), window(2, true, 100, 0)];
        windows.sort_by_key(|w| std::cmp::Reverse(w.sort_key()));
        assert_eq!(
            windows[0].id, 2,
            "is_focused must outrank a newer focus_timestamp on another window"
        );

        // A never-focused window sorts last, so it can't hijack the target.
        let never = NiriWindow {
            id: 9,
            title: None,
            app_id: None,
            workspace_id: None,
            is_focused: false,
            focus_timestamp: None,
        };
        let mut windows = [never, window(1, false, 1, 0)];
        windows.sort_by_key(|w| std::cmp::Reverse(w.sort_key()));
        assert_eq!(
            windows[0].id, 1,
            "a never-focused window must sort behind any focused-at-least-once window"
        );
    }
}
