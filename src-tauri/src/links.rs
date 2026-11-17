//! The few web pages About may open (M3a Task 11): the project page, the
//! changelog, the issue tracker and the releases page. The window names one;
//! it never passes a URL, so it cannot open anything else. The page opens in
//! the system browser; the app itself fetches nothing (OPS-02: no automatic
//! internet lookup).

use std::process::Command;

const PROJECT: &str = "https://github.com/armysarge/mercurychat";

/// The URL for a link name, or `None` for anything not on the list.
pub fn url_for(name: &str) -> Option<String> {
    let path = match name {
        "project" => "",
        "changelog" => "/blob/main/CHANGELOG.md",
        "issues" => "/issues",
        "releases" => "/releases",
        _ => return None,
    };
    Some(format!("{PROJECT}{path}"))
}

/// Hand `url` to the operating system's browser. Lab builds (debug only) can
/// record the URL in the file `MC_OPEN_LINK_LOG` names instead, so the UI
/// check sees what would open without starting a browser.
pub fn open(url: &str) -> std::io::Result<()> {
    #[cfg(debug_assertions)]
    if let Some(log) = std::env::var_os("MC_OPEN_LINK_LOG") {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(log)?;
        return writeln!(f, "{url}");
    }
    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", url]);
        c
    } else if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(url);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    cmd.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_four_named_links_resolve() {
        assert_eq!(url_for("project").as_deref(), Some("https://github.com/armysarge/mercurychat"));
        assert_eq!(
            url_for("changelog").as_deref(),
            Some("https://github.com/armysarge/mercurychat/blob/main/CHANGELOG.md")
        );
        assert_eq!(url_for("issues").as_deref(), Some("https://github.com/armysarge/mercurychat/issues"));
        assert_eq!(url_for("releases").as_deref(), Some("https://github.com/armysarge/mercurychat/releases"));
        for other in ["", "Project", "https://example.com", "file:///C:/Windows", "../issues", "issues "] {
            assert_eq!(url_for(other), None, "{other:?} must not resolve");
        }
    }
}
