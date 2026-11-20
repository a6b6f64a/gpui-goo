use std::cell::RefCell;
use std::io::Cursor;
use std::net::SocketAddr;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use gpui::InputEvent as _;
use gpui::MouseDownEvent;
use gpui::MouseMoveEvent;
use gpui::MouseUpEvent;
use gpui::Window;
use gpui::test::TestQueryExt as _;
use gpui::test::TestWindowExt as _;

use crate::protocol::Command;
use crate::protocol::Node;
use crate::protocol::Response;
use crate::server::Server;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("NATIVE_UI_ADDR must be a loopback socket address")]
    Address(#[source] std::net::AddrParseError),
    #[error("NATIVE_UI_TOKEN is required when automation is enabled")]
    Token,
    #[error(transparent)]
    Server(#[from] crate::server::Error),
}

/// Starts an opt-in bridge for one existing window. Does nothing without NATIVE_UI_ADDR.
pub fn attach(handle: gpui::AnyWindowHandle, cx: &mut gpui::App) -> Result<(), Error> {
    let Some(address) = std::env::var_os("NATIVE_UI_ADDR") else {
        return Ok(());
    };
    let address: SocketAddr = address.to_string_lossy().parse().map_err(Error::Address)?;
    let token = std::env::var("NATIVE_UI_TOKEN").map_err(|_| Error::Token)?;
    let server = Server::bind(address, token)?;
    let (sender, receiver) =
        async_channel::bounded::<(Command, Instant, mpsc::SyncSender<Response>)>(1);
    let dispatch = sender.clone();
    let running = server.start(move |command| {
        let (reply, result) = mpsc::sync_channel(1);
        if dispatch
            .try_send((command, Instant::now() + Duration::from_secs(5), reply))
            .is_err()
        {
            return failure(
                "ui_busy",
                "UI bridge has stopped or has an outstanding command",
            );
        }
        result
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or_else(|_| failure("ui_timeout", "UI did not answer within five seconds"))
    })?;
    let owner = Rc::new(RefCell::new(Some(running)));
    let close_owner = owner.clone();
    let close_sender = sender.clone();
    cx.on_window_closed(move |_, id| {
        if id == handle.window_id() {
            close_sender.close();
            close_owner.borrow_mut().take();
        }
    })
    .detach();
    cx.on_app_quit(move |_| {
        sender.close();
        owner.borrow_mut().take();
        async {}
    })
    .detach();
    cx.spawn(async move |cx| {
        while let Ok((command, deadline, reply)) = receiver.recv().await {
            if matches!(command, Command::Screenshot) {
                let (presented, ready) = async_channel::bounded(1);
                if handle
                    .update(cx, |_, window, cx| {
                        window.render_frame(cx);
                        window.on_next_frame(move |_, _| {
                            let _ = presented.try_send(());
                        });
                    })
                    .is_ok()
                {
                    let mut timeout = std::pin::pin!(
                        cx.background_executor()
                            .timer(deadline.saturating_duration_since(Instant::now()))
                    );
                    let mut ready = std::pin::pin!(ready.recv());
                    std::future::poll_fn(|cx| {
                        if timeout.as_mut().poll(cx).is_ready() {
                            return std::task::Poll::Ready(());
                        }
                        ready.as_mut().poll(cx).map(|_| ())
                    })
                    .await;
                }
            }
            let response = if Instant::now() >= deadline {
                failure("ui_timeout", "Command expired before UI dispatch")
            } else {
                match command {
                    Command::Input { text } => {
                        let mut response = Response::Done;
                        for character in text.chars() {
                            if Instant::now() >= deadline {
                                response = failure("ui_timeout", "Input expired during dispatch");
                                break;
                            }
                            response = handle
                                .update(cx, |_, window, cx| {
                                    execute(
                                        Command::Input {
                                            text: character.to_string(),
                                        },
                                        window,
                                        cx,
                                    )
                                })
                                .unwrap_or_else(|_| {
                                    failure("window_closed", "Window no longer exists")
                                });
                            if matches!(response, Response::Error { .. }) {
                                break;
                            }
                        }
                        response
                    }
                    command => handle
                        .update(cx, |_, window, cx| execute(command, window, cx))
                        .unwrap_or_else(|_| failure("window_closed", "Window no longer exists")),
                }
            };
            let _ = reply.send(response);
        }
    })
    .detach();
    Ok(())
}

fn failure(code: &str, message: &str) -> Response {
    Response::Error {
        code: code.into(),
        message: message.into(),
    }
}

fn snapshot(window: &Window) -> Vec<Node> {
    window
        .elements()
        .into_iter()
        .map(|element| {
            let bounds = element.bounds();
            Node {
                path: element.path().iter().map(ToString::to_string).collect(),
                role: element.role().map(|role| format!("{role:?}")),
                label: element.label().map(str::to_owned),
                value: element.value().map(str::to_owned),
                checked: element.checked(),
                visible: element.visible(),
                disabled: element.disabled(),
                bounds: [
                    bounds.origin.x.into(),
                    bounds.origin.y.into(),
                    bounds.size.width.into(),
                    bounds.size.height.into(),
                ],
            }
        })
        .collect()
}

fn target_position(
    selector: &crate::protocol::Selector,
    window: &Window,
) -> Result<gpui::Point<gpui::Pixels>, Response> {
    let nodes = snapshot(window);
    let node = crate::protocol::unique(&nodes, selector)
        .map_err(|code| failure(code, "Selector must identify exactly one observed element"))?;
    if !node.visible
        || node.disabled == Some(true)
        || node.bounds.iter().any(|n| !n.is_finite())
        || node.bounds[2] <= 0.0
        || node.bounds[3] <= 0.0
    {
        return Err(failure(
            "not_actionable",
            "Element is hidden, disabled or has no painted bounds",
        ));
    }
    Ok(gpui::point(
        gpui::px(node.bounds[0] + node.bounds[2] / 2.0),
        gpui::px(node.bounds[1] + node.bounds[3] / 2.0),
    ))
}

fn execute(command: Command, window: &mut Window, cx: &mut gpui::App) -> Response {
    window.render_frame(cx);
    match command {
        Command::Snapshot => Response::Nodes {
            nodes: snapshot(window),
        },
        Command::Click { selector } => {
            let position = match target_position(&selector, window) {
                Ok(position) => position,
                Err(response) => return response,
            };
            let modifiers = window.modifiers();
            window.dispatch_event(
                MouseMoveEvent {
                    position,
                    pressed_button: None,
                    modifiers,
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
            window.dispatch_event(
                MouseDownEvent {
                    position,
                    button: gpui::MouseButton::Left,
                    modifiers,
                    click_count: 1,
                    first_mouse: false,
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
            window.dispatch_event(
                MouseUpEvent {
                    position,
                    button: gpui::MouseButton::Left,
                    modifiers,
                    click_count: 1,
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
            Response::Done
        }
        Command::Scroll { selector, delta } => {
            let position = match target_position(&selector, window) {
                Ok(position) => position,
                Err(response) => return response,
            };
            window.dispatch_event(
                gpui::ScrollEvent {
                    position,
                    delta: gpui::ScrollDelta::Pixels(gpui::point(
                        gpui::px(-delta[0]),
                        gpui::px(-delta[1]),
                    )),
                    modifiers: window.modifiers(),
                    touch_phase: gpui::TouchPhase::Moved,
                    source: gpui::ScrollSource::Pointer,
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
            Response::Done
        }
        Command::Input { text } => {
            let mut key = gpui::Keystroke::parse_with_os(&text, cx.platform_info().os)
                .expect("input command contains one Unicode character");
            key.key_char = Some(text);
            window.dispatch_keystroke(key, cx);
            Response::Done
        }
        Command::Press { key } => {
            let Ok(key) = gpui::Keystroke::parse_with_os(&key, cx.platform_info().os) else {
                return failure("invalid_key", "Key is not a valid GPUI keystroke");
            };
            let key = if matches!(key.key.as_str(), "enter" | "tab") {
                window.dispatch_event(
                    gpui::KeyDownEvent {
                        keystroke: key.clone(),
                        is_held: false,
                        prefer_character_input: false,
                    }
                    .to_platform_input(),
                    cx,
                );
                key
            } else {
                let key = key.with_simulated_ime();
                window.dispatch_keystroke(key.clone(), cx);
                key
            };
            window.dispatch_event(gpui::KeyUpEvent { keystroke: key }.to_platform_input(), cx);
            Response::Done
        }
        Command::Screenshot => {
            let image = match capture(window) {
                Ok(image) => image,
                Err(response) => return response,
            };
            let mut png = Cursor::new(Vec::new());
            if let Err(error) = image.write_to(&mut png, image::ImageFormat::Png) {
                return failure("screenshot_encoding", &error.to_string());
            }
            Response::Png {
                base64: base64::engine::general_purpose::STANDARD.encode(png.into_inner()),
            }
        }
    }
}

fn capture(window: &Window) -> Result<image::RgbaImage, Response> {
    match window.render_to_image() {
        Ok(image) => Ok(image),
        Err(error) => {
            #[cfg(target_os = "linux")]
            {
                capture_x11(window).map_err(|capture| {
                    failure(
                        "screenshot_unavailable",
                        &format!("GPU capture: {error}; X11 capture: {capture}"),
                    )
                })
            }
            #[cfg(not(target_os = "linux"))]
            {
                Err(failure("screenshot_unavailable", &error.to_string()))
            }
        }
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug, thiserror::Error)]
enum CaptureError {
    #[error(transparent)]
    Handle(#[from] raw_window_handle::HandleError),
    #[error(transparent)]
    WindowId(#[from] std::num::TryFromIntError),
    #[error(transparent)]
    Connect(#[from] x11rb::errors::ConnectError),
    #[error(transparent)]
    Connection(#[from] x11rb::errors::ConnectionError),
    #[error(transparent)]
    Reply(#[from] x11rb::errors::ReplyError),
    #[error("{0}")]
    Unsupported(&'static str),
}

#[cfg(target_os = "linux")]
fn capture_x11(window: &Window) -> Result<image::RgbaImage, CaptureError> {
    use raw_window_handle::RawWindowHandle;
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::ConnectionExt as _;
    use x11rb::protocol::xproto::ImageFormat;
    use x11rb::protocol::xproto::ImageOrder;
    let id = match raw_window_handle::HasWindowHandle::window_handle(window)?.as_raw() {
        RawWindowHandle::Xlib(handle) => u32::try_from(handle.window)?,
        RawWindowHandle::Xcb(handle) => handle.window.get(),
        _ => {
            return Err(CaptureError::Unsupported(
                "Linux capture requires an X11 window; run test worker under Xvfb",
            ));
        }
    };
    let (connection, _) = x11rb::connect(None)?;
    let geometry = connection.get_geometry(id)?.reply()?;
    let setup = connection.setup();
    let format = setup
        .pixmap_formats
        .iter()
        .find(|format| format.depth == geometry.depth)
        .ok_or(CaptureError::Unsupported("X11 pixmap format missing"))?;
    if setup.image_byte_order != ImageOrder::LSB_FIRST || format.bits_per_pixel != 32 {
        return Err(CaptureError::Unsupported(
            "X11 capture requires little-endian 32-bit pixel storage",
        ));
    }
    let shot = connection
        .get_image(
            ImageFormat::Z_PIXMAP,
            id,
            0,
            0,
            geometry.width,
            geometry.height,
            u32::MAX,
        )?
        .reply()?;
    let visual = setup
        .roots
        .iter()
        .flat_map(|root| &root.allowed_depths)
        .flat_map(|depth| &depth.visuals)
        .find(|visual| visual.visual_id == shot.visual)
        .ok_or(CaptureError::Unsupported("X11 visual missing"))?;
    if (visual.red_mask, visual.green_mask, visual.blue_mask) != (0xff0000, 0xff00, 0xff) {
        return Err(CaptureError::Unsupported(
            "X11 visual is not supported RGB888",
        ));
    }
    let mut pixels = shot.data;
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
        pixel[3] = 255;
    }
    image::RgbaImage::from_raw(geometry.width.into(), geometry.height.into(), pixels)
        .ok_or(CaptureError::Unsupported("X11 image length mismatch"))
}
