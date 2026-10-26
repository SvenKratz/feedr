use crate::model::{Post, Resource, ResourceKind};
use anyhow::{Result, ensure};
use std::{path::Path, process::Command};

pub fn open_resource(post: &Post, r: &Resource) -> Result<()> {
    if r.kind == ResourceKind::Url {
        let url = url::Url::parse(&r.target)?;
        ensure!(
            ["http", "https"].contains(&url.scheme()),
            "Unsupported URL scheme"
        );
    } else {
        ensure!(
            Path::new(&r.target).is_absolute(),
            "This resource location is unresolved"
        );
        ensure!(
            post.context.is_local(),
            "This resource is on another machine; remote retrieval is not available yet"
        );
        ensure!(
            Path::new(&r.target).exists(),
            "Resource is unavailable: {}",
            r.target
        );
    }
    open::that_detached(&r.target)?;
    Ok(())
}
pub fn reveal(post: &Post, r: &Resource) -> Result<()> {
    ensure!(
        r.kind != ResourceKind::Url,
        "URLs do not have an enclosing folder"
    );
    ensure!(
        post.context.is_local(),
        "Remote folders require a mounted path"
    );
    let path = Path::new(&r.target);
    ensure!(path.is_absolute(), "This resource location is unresolved");
    let folder = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    ensure!(folder.is_dir(), "Enclosing folder is unavailable");
    #[cfg(target_os = "macos")]
    {
        if path.exists() && !path.is_dir() {
            Command::new("/usr/bin/open").arg("-R").arg(path).spawn()?;
        } else {
            open::that_detached(folder)?;
        }
    }
    #[cfg(target_os = "linux")]
    {
        if path.exists() && !path.is_dir() {
            let uri =
                url::Url::from_file_path(path).map_err(|_| anyhow::anyhow!("Invalid path"))?;
            let status = Command::new("dbus-send")
                .args([
                    "--session",
                    "--dest=org.freedesktop.FileManager1",
                    "--type=method_call",
                    "/org/freedesktop/FileManager1",
                    "org.freedesktop.FileManager1.ShowItems",
                ])
                .arg(format!("array:string:{uri}"))
                .arg("string:")
                .status();
            if status.is_ok_and(|s| s.success()) {
                return Ok(());
            }
        }
        open::that_detached(folder)?;
    }
    Ok(())
}
