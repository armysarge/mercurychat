//! A development build (one built without `tauri/custom-protocol`, e.g. by a
//! plain `cargo build` or `cargo run`) loads its pages from the Vite dev
//! server at `devUrl`. When that server is not running, the window would show
//! the browser's own "refused to connect" page and the app would seem to hang
//! (owner report, 17 Nov). Instead the app checks first and, if nothing
//! answers, opens one ordinary window that says what happened and how to run
//! a build that works. Release builds never check: their pages are built in.

use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;
use tauri::Url;

/// How long to wait for the dev server to accept a connection.
pub const TIMEOUT: Duration = Duration::from_millis(300);

/// True when something accepts a TCP connection at the URL's host and port.
pub fn reachable(url: &Url, timeout: Duration) -> bool {
    let Some(host) = url.host_str() else { return false };
    let Some(port) = url.port_or_known_default() else { return false };
    let Ok(addrs) = (host, port).to_socket_addrs() else { return false };
    addrs.into_iter().any(|a| TcpStream::connect_timeout(&a, timeout).is_ok())
}

/// The page shown instead of the splash, as HTML.
pub fn page(url: &Url) -> String {
    let at = escape(url.as_str());
    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><title>MercuryChat: dev server not running</title>
<style>
:root {{ color-scheme: light dark; }}
body {{ font: 14px/1.5 system-ui, sans-serif; margin: 24px; background: Canvas; color: CanvasText; }}
h1 {{ font-size: 18px; margin: 0 0 8px; }}
code {{ font: 13px ui-monospace, Consolas, monospace; padding: 1px 4px; border: 1px solid GrayText; border-radius: 4px; }}
li {{ margin: 6px 0; }}
</style></head><body>
<h1>This development build needs its dev server</h1>
<p>It was built without its pages, so it loads them from <code>{at}</code>, and nothing is answering there.</p>
<p>Any one of these works:</p>
<ul>
<li><code>npm run tauri dev</code> starts the dev server and the app together.</li>
<li><code>npx tauri build --debug</code> builds an app with its pages inside.</li>
<li><code>cargo build -p mercurychat --features tauri/custom-protocol</code> does the same with cargo (build the UI first: <code>npm run build:ui</code>).</li>
</ul>
<p>Close this window to quit. Nothing else was started: no station data was opened and no modem was touched.</p>
</body></html>"#
    )
}

/// Write the page to a file in the temp folder and return its `file:` URL
/// (Tauri loads `data:` URLs only with an extra feature and crate).
pub fn page_file(url: &Url) -> Option<Url> {
    let path = std::env::temp_dir().join("mercurychat-dev-server-missing.html");
    std::fs::write(&path, page(url)).ok()?;
    Url::from_file_path(&path).ok()
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn a_listening_port_is_reachable_and_a_closed_one_is_not() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let open = Url::parse(&format!("http://127.0.0.1:{port}")).unwrap();
        assert!(reachable(&open, TIMEOUT));
        drop(listener);
        assert!(!reachable(&open, TIMEOUT), "nothing listens once the listener is gone");
    }

    #[test]
    fn the_page_names_the_url_and_the_commands_and_escapes_html() {
        let url = Url::parse("http://127.0.0.1:5173/?a=<b>").unwrap();
        let html = page(&url);
        assert!(html.contains("http://127.0.0.1:5173/?a=%3Cb%3E"), "{html}");
        assert!(!html.contains("<b>"));
        for cmd in ["npm run tauri dev", "npx tauri build --debug", "--features tauri/custom-protocol"] {
            assert!(html.contains(cmd), "missing {cmd}");
        }
        let file = page_file(&url).expect("a file: URL");
        assert_eq!(file.scheme(), "file");
        let written = std::fs::read_to_string(file.to_file_path().unwrap()).unwrap();
        assert_eq!(written, html);
    }
}
