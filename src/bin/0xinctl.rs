use std::env;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

fn socket_path() -> Result<PathBuf, String> {
    let runtime = env::var_os("XDG_RUNTIME_DIR")
        .ok_or_else(|| "XDG_RUNTIME_DIR is not set".to_string())?;
    let display = env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "default".into());
    let safe_display = display.replace('/', "_");
    Ok(PathBuf::from(runtime).join(format!("0xin-control-{safe_display}.sock")))
}

fn usage() -> ! {
    eprintln!("usage: 0xinctl wallpaper PATH\n       0xinctl wallpaper clear");
    std::process::exit(2);
}

fn main() {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        usage();
    };
    let Some(argument) = args.next() else {
        usage();
    };
    if args.next().is_some() || command != "wallpaper" {
        usage();
    }

    let request = if argument == "clear" {
        "wallpaper clear\n".to_string()
    } else {
        format!("wallpaper {argument}\n")
    };
    let path = socket_path().unwrap_or_else(|error| {
        eprintln!("0xinctl: {error}");
        std::process::exit(1);
    });
    let mut stream = UnixStream::connect(&path).unwrap_or_else(|error| {
        eprintln!("0xinctl: cannot connect to {}: {error}", path.display());
        std::process::exit(1);
    });
    stream.write_all(request.as_bytes()).unwrap_or_else(|error| {
        eprintln!("0xinctl: failed to send request: {error}");
        std::process::exit(1);
    });
    stream.shutdown(std::net::Shutdown::Write).ok();

    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap_or_else(|error| {
        eprintln!("0xinctl: failed to read response: {error}");
        std::process::exit(1);
    });
    print!("{response}");
    if !response.starts_with("ok") {
        std::process::exit(1);
    }
}
