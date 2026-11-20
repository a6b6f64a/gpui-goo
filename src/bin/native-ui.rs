use gpui_goo::client::Client;
use gpui_goo::protocol::Selector;
use std::net::SocketAddr;
use std::path::Path;
use std::process::ExitCode;

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("{0}")]
    Usage(&'static str),
    #[error("NATIVE_UI_TOKEN is required")]
    Token,
    #[error(transparent)]
    Address(#[from] std::net::AddrParseError),
    #[error(transparent)]
    Client(#[from] gpui_goo::client::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Delta(#[from] std::num::ParseFloatError),
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("native-ui: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Error> {
    let mut args = std::env::args().skip(1);
    let address: SocketAddr = args
        .next()
        .ok_or(Error::Usage("usage: native-ui ADDRESS COMMAND [ARG...]"))?
        .parse()?;
    let command = args.next().ok_or(Error::Usage("missing command"))?;
    let token = std::env::var("NATIVE_UI_TOKEN").map_err(|_| Error::Token)?;
    let mut client = Client::connect(address, token)?;
    match command.as_str() {
        "snapshot" if args.next().is_none() => {
            println!("{}", serde_json::to_string_pretty(&client.snapshot()?)?)
        }
        "click" => {
            let id = one_arg(args)?;
            client.click(Selector::id(id))?;
        }
        "scroll" => {
            let id = args.next().ok_or(Error::Usage("missing scroll target"))?;
            let x = args
                .next()
                .ok_or(Error::Usage("missing horizontal delta"))?
                .parse()?;
            let y = one_arg(args)?.parse()?;
            client.scroll(Selector::id(id), [x, y])?;
        }
        "input" => {
            let text = one_arg(args)?;
            client.input(&text)?;
        }
        "press" => {
            let key = one_arg(args)?;
            client.press(&key)?;
        }
        "screenshot" => {
            let path = one_arg(args)?;
            client.screenshot(Path::new(&path))?;
        }
        _ => {
            return Err(Error::Usage(
                "usage: native-ui ADDRESS snapshot|click ID|scroll ID DX DY|input TEXT|press KEY|screenshot PATH",
            ));
        }
    }
    Ok(())
}

fn one_arg(mut args: impl Iterator<Item = String>) -> Result<String, Error> {
    let arg = args
        .next()
        .ok_or(Error::Usage("missing command argument"))?;
    if args.next().is_some() {
        return Err(Error::Usage("unexpected extra argument"));
    }
    Ok(arg)
}
