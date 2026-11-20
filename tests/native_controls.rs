#![cfg(feature = "gpui")]

use gpui_goo::client::Client;
use gpui_goo::protocol::Selector;
use std::net::{SocketAddr, TcpListener};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "requires built controls example and native display; set GPUI_GOO_DEMO and DISPLAY"]
fn controls_toggle_changes_real_window_and_captures_png() {
    let demo =
        std::env::var("GPUI_GOO_DEMO").expect("set GPUI_GOO_DEMO to built controls executable");
    #[cfg(target_os = "linux")]
    assert!(
        !std::env::var_os("DISPLAY").unwrap_or_default().is_empty(),
        "DISPLAY must name native screen"
    );
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve loopback port");
    let address = listener.local_addr().unwrap();
    drop(listener);
    let address: SocketAddr = address;
    let token = "gpui-goo-native-controls-integration-token";
    let mut child = OwnedChild(
        Command::new(demo)
            .env("NATIVE_UI_ADDR", address.to_string())
            .env("NATIVE_UI_TOKEN", token)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start controls demo"),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut client = loop {
        if let Some(status) = child.0.try_wait().expect("check demo process") {
            panic!("controls demo exited: {status}");
        }
        match Client::connect(address, token.into()) {
            Ok(client) => break client,
            Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(100)),
            Err(error) => panic!("controls demo did not start before deadline: {error}"),
        }
    };
    let initial = client.snapshot().expect("read initial real UI");
    let state = Selector::id("state");
    let toggle = Selector::id("toggle");
    let before = gpui_goo::protocol::unique(&initial, &state).expect("state node");
    assert!(
        gpui_goo::protocol::unique(&initial, &toggle)
            .expect("toggle node")
            .visible
    );
    client.click(toggle).expect("click real toggle");
    let changed = client
        .wait_visible(&state, Duration::from_secs(5))
        .expect("state remains visible");
    let name_selector = Selector {
        id: None,
        role: Some("TextInput".into()),
        label: Some("Name".into()),
    };
    client
        .click(name_selector.clone())
        .expect("focus real text input");
    client.input("Odyssey").expect("type into focused widget");
    client
        .press("backspace")
        .expect("dispatch native key event");
    let nodes = client.snapshot().expect("read edited input");
    let name = gpui_goo::protocol::unique(&nodes, &name_selector).expect("name input");
    expect_test::expect![[r#"
        toggle: off -> on
        name: Odysse
    "#]]
    .assert_eq(&format!(
        "toggle: {} -> {}\nname: {}\n",
        before.label.as_deref().unwrap_or("<missing>"),
        changed.label.as_deref().unwrap_or("<missing>"),
        name.value.as_deref().unwrap_or("<missing>"),
    ));
    let artifact = std::env::var_os("GPUI_GOO_SCREENSHOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("gpui-goo-native-controls.png"));
    client
        .screenshot(&artifact)
        .expect("capture real window PNG");
    let png = std::fs::read(&artifact).expect("read screenshot artifact");
    assert!(
        png.starts_with(b"\x89PNG\r\n\x1a\n"),
        "screenshot artifact must be PNG"
    );
    eprintln!("native controls screenshot: {}", artifact.display());
}
