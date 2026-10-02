//! Private-session-bus fixture. It never opens files or contacts USB hardware.

use std::{
    collections::HashMap,
    io::{self, Write},
};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, connection::Builder, message::Header};

type Filter = (String, Vec<(u32, String)>);
type Options = HashMap<String, OwnedValue>;

struct FileChooser {
    requests: usize,
}

fn required<T>(options: &mut Options, key: &str) -> zbus::fdo::Result<T>
where
    T: TryFrom<OwnedValue>,
    T::Error: std::fmt::Display,
{
    let value = options
        .remove(key)
        .ok_or_else(|| zbus::fdo::Error::InvalidArgs(format!("Missing {key}")))?;
    T::try_from(value)
        .map_err(|error| zbus::fdo::Error::InvalidArgs(format!("Invalid {key}: {error}")))
}

fn ensure(condition: bool, message: &str) -> zbus::fdo::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(zbus::fdo::Error::InvalidArgs(message.into()))
    }
}

impl FileChooser {
    async fn choose(
        &mut self,
        save: bool,
        parent: &str,
        title: &str,
        mut options: Options,
        header: Header<'_>,
        connection: &Connection,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        ensure(parent.is_empty(), "Parent window must be empty")?;
        ensure(title == "Captura ñ 💡", "Unicode title changed")?;
        let token: String = required(&mut options, "handle_token")?;
        ensure(
            !token.is_empty()
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'),
            "Invalid handle token",
        )?;
        ensure(
            required::<bool>(&mut options, "modal")?,
            "Request must be modal",
        )?;
        let filters: Vec<Filter> = required(&mut options, "filters")?;
        ensure(filters.len() == 2, "Expected two filters")?;
        let initial: Filter = required(&mut options, "current_filter")?;
        ensure(initial == filters[0], "Initial filter changed")?;
        if save {
            ensure(
                required::<String>(&mut options, "current_name")? == "señal 💡.png",
                "Unicode name changed",
            )?;
            ensure(
                required::<String>(&mut options, "accept_label")? == "Guardar ñ 💡",
                "Unicode accept label changed",
            )?;
            let mut expected_folder = "/tmp/carpeta ñ 💡".as_bytes().to_vec();
            expected_folder.push(0);
            ensure(
                required::<Vec<u8>>(&mut options, "current_folder")? == expected_folder,
                "Current folder must be a NUL-terminated UTF-8 byte array",
            )?;
            ensure(
                !options.contains_key("multiple") && !options.contains_key("directory"),
                "Save request must omit open-only flags",
            )?;
        } else {
            ensure(
                required::<bool>(&mut options, "multiple")?,
                "Expected multiple open selection",
            )?;
            ensure(
                !required::<bool>(&mut options, "directory")?,
                "Expected files, not directories",
            )?;
            ensure(
                !options.contains_key("current_name"),
                "Open request must omit current_name",
            )?;
        }
        ensure(options.is_empty(), "Unexpected chooser option")?;
        let sender = header
            .sender()
            .ok_or_else(|| zbus::fdo::Error::Failed("Missing sender".into()))?;
        let peer = sender
            .as_str()
            .strip_prefix(':')
            .unwrap_or(sender.as_str())
            .replace('.', "_");
        let request = OwnedObjectPath::try_from(format!(
            "/org/freedesktop/portal/desktop/request/{peer}/{token}"
        ))
        .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
        let mut results = HashMap::<&str, Value<'_>>::new();
        if self.requests != 2 {
            let suffix = if self.requests == 3 { "png" } else { "csv" };
            let mut uris = vec![format!(
                "file:///tmp/document-grant/se%C3%B1al%20%C3%B1%20%F0%9F%92%A1.{suffix}"
            )];
            if !save {
                uris.push("file:///tmp/document-grant/second.csv".into());
            }
            results.insert("uris", Value::from(uris));
            let mut selected = filters[if self.requests == 3 { 0 } else { 1 }].clone();
            if self.requests >= 3 {
                // Reproduce KDE's normalization, an unknown format, and duplicate formats.
                // https://github.com/KDE/xdg-desktop-portal-kde/blob/v6.6.6/src/filechooser.cpp
                let pattern = match self.requests {
                    3 => "*.png",
                    5 => "*.xls",
                    _ => "*.csv",
                };
                selected.1 = vec![(0, pattern.into())];
            }
            results.insert("current_filter", Value::from(selected));
        }
        let response = if self.requests == 2 { 1_u32 } else { 0_u32 };
        connection
            .emit_signal(
                Some(sender.as_str()),
                &request,
                "org.freedesktop.portal.Request",
                "Response",
                &(response, results),
            )
            .await
            .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
        self.requests += 1;
        Ok(request)
    }
}

#[zbus::interface(name = "org.freedesktop.portal.FileChooser")]
impl FileChooser {
    async fn save_file(
        &mut self,
        parent: &str,
        title: &str,
        options: Options,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        self.choose(true, parent, title, options, header, connection)
            .await
    }

    async fn open_file(
        &mut self,
        parent: &str,
        title: &str,
        options: Options,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        self.choose(false, parent, title, options, header, connection)
            .await
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _connection = Builder::session()?
        .name("org.freedesktop.portal.Desktop")?
        .serve_at(
            "/org/freedesktop/portal/desktop",
            FileChooser { requests: 0 },
        )?
        .build()
        .await?;
    println!("ready");
    io::stdout().flush()?;
    std::future::pending::<()>().await;
    Ok(())
}
