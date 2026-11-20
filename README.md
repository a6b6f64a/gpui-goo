# gpui-goo

UI automation for GPUI applications: inspect controls, send input, and capture window screenshots.

## Getting started

Requires a GPUI Kit 0.9.1 application, Rust 2024 support, and GPUI's native build prerequisites.

```sh
cargo add gpui-goo --git https://github.com/Abrahamh08/gpui-goo --features gpui
```

Attach to an existing window after opening it:

```rust
gpui_goo::gpui_adapter::attach(window.into(), cx)?;
```

Built-in Kit controls are observed automatically. For custom controls, import `gpui::TestSupportExt`, assign stable IDs, and call `.test_support()`. See [controls example](examples/controls.rs).

Enable automation for the application and client in the same shell:

```sh
export NATIVE_UI_ADDR=127.0.0.1:57123
export NATIVE_UI_TOKEN="$(openssl rand -hex 32)"
```

Without `NATIVE_UI_ADDR`, attachment does nothing. Start your application normally, then use the client API or CLI:

```sh
cargo install --git https://github.com/Abrahamh08/gpui-goo --bin native-ui
native-ui 127.0.0.1:57123 snapshot
native-ui 127.0.0.1:57123 click save
native-ui 127.0.0.1:57123 scroll content 0 500
native-ui 127.0.0.1:57123 screenshot window.png
```

Selectors must identify one control. Scroll deltas are pixels, positive right/down. Text input goes to the focused control. Composite inputs expose separate frame and editable nodes; use role/label selectors through the client API to inspect editable values.

## Example and tests

From a checkout:

```sh
git clone https://github.com/Abrahamh08/gpui-goo
cd gpui-goo
cargo install cargo-nextest --locked
cargo build --features gpui --example controls
cargo nextest run --features gpui
```

The graphical test launches its own window, checks interactions, and saves a screenshot. Run on a native display or isolated Xvfb display:

```sh
mkdir -p artifacts
GPUI_GOO_DEMO="$PWD/target/debug/examples/controls" \
GPUI_GOO_SCREENSHOT="$PWD/artifacts/controls.png" \
cargo nextest run --features gpui --profile gui --run-ignored only
```

GUI tests run serially. Review changed expectations with `UPDATE_EXPECT=1`; screenshots are review artifacts, not pixel-diff assertions. Run doctests separately with `cargo test --doc`.

## Platform and security

Linux capture requires X11; Wayland capture is unavailable. Use release-profile builds for portable Windows artifacts: debug builds load GPUI shaders from the source tree. macOS is not yet verified. Automation covers the application's GPUI window, not OS dialogs or other processes.

Keep the bridge out of production releases. Use a fresh secret of at least 32 bytes; never log or commit it. Loopback authentication is not a sandbox against same-user processes. A timed-out action may already have taken effect; do not blindly retry it.
