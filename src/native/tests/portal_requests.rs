//! Exercise actual D-Bus ordering and FD ownership on isolated, hardware-free buses.

use std::{
    fs,
    io::{BufRead, BufReader},
    os::fd::OwnedFd,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::runtime::Runtime;
use vdsportal::portal::{Portal, PortalError, Results};
use zbus::zvariant::{Fd, OwnedObjectPath, OwnedValue, Str, Value};
use zbus::{Connection, connection::Builder, message::Header};

const NAME: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const REQUEST: &str = "org.freedesktop.portal.Request";
const CHOOSER: &str = "org.freedesktop.portal.FileChooser";
static NEXT_BUS: AtomicU64 = AtomicU64::new(1);

struct PrivateBus {
    child: Child,
    directory: PathBuf,
    address: String,
}

impl PrivateBus {
    fn start() -> Self {
        let sequence = NEXT_BUS.fetch_add(1, Ordering::Relaxed);
        let directory =
            std::env::temp_dir().join(format!("vdsportal-test-{}-{sequence}", std::process::id()));
        fs::create_dir(&directory).expect("create private bus directory");
        let config = directory.join("bus.conf");
        fs::write(&config, r#"<busconfig><type>session</type><listen>unix:tmpdir=/tmp</listen><auth>EXTERNAL</auth><policy context="default"><allow send_destination="*"/><allow receive_sender="*"/><allow own="*"/></policy></busconfig>"#).expect("write private bus config");
        let daemon = std::env::var_os("DBUS_DAEMON").unwrap_or_else(|| "dbus-daemon".into());
        let mut child = Command::new(daemon)
            .arg(format!("--config-file={}", config.display()))
            .args(["--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("start private dbus-daemon");
        let mut address = String::new();
        BufReader::new(child.stdout.take().expect("daemon stdout"))
            .read_line(&mut address)
            .expect("read bus address");
        assert!(
            !address.trim().is_empty(),
            "private bus exited without an address"
        );
        Self {
            child,
            directory,
            address: address.trim().into(),
        }
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[derive(Clone, Copy)]
enum ResponseMode {
    Early,
    Late,
    Cancel,
    Failed,
    WrongPath,
    Malformed,
    Silent,
    ServiceLoss,
    Replacement,
    Rogue,
}

#[derive(Clone, Copy)]
enum UsbMode {
    Valid,
    WrongId,
    Duplicate,
    MissingFd,
    WrongFdType,
    MissingDevice,
    Denied,
    Cancel,
}

#[derive(Default)]
struct Calls {
    senders: Vec<String>,
    finish: usize,
    release: Vec<String>,
}

struct Chooser {
    mode: ResponseMode,
    rogue: Connection,
    calls: Arc<Mutex<Calls>>,
}

// Derive the predicted request path from the documented handle_token protocol.
// https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Request.html
fn request_path(header: &Header<'_>, options: &mut Results) -> OwnedObjectPath {
    let sender = header.sender().expect("request sender").as_str();
    let token = String::try_from(options.remove("handle_token").expect("handle_token"))
        .expect("string token");
    let peer = sender
        .strip_prefix(':')
        .expect("unique sender")
        .replace('.', "_");
    OwnedObjectPath::try_from(format!("{PATH}/request/{peer}/{token}"))
        .expect("predicted request path")
}

async fn respond(connection: &Connection, sender: &str, path: &OwnedObjectPath, code: u32) {
    connection
        .emit_signal(
            Some(sender),
            path,
            REQUEST,
            "Response",
            &(code, Results::new()),
        )
        .await
        .expect("emit response");
}

#[zbus::interface(name = "org.freedesktop.portal.FileChooser")]
impl Chooser {
    async fn save_file(
        &self,
        _parent: &str,
        _title: &str,
        mut options: Results,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> OwnedObjectPath {
        let path = request_path(&header, &mut options);
        let sender = header.sender().expect("sender").to_string();
        self.calls
            .lock()
            .expect("calls lock")
            .senders
            .push(sender.clone());
        match self.mode {
            ResponseMode::Early | ResponseMode::WrongPath => {
                respond(connection, &sender, &path, 0).await
            }
            ResponseMode::Cancel => respond(connection, &sender, &path, 1).await,
            ResponseMode::Failed => respond(connection, &sender, &path, 2).await,
            ResponseMode::Malformed => connection
                .emit_signal(
                    Some(sender.as_str()),
                    &path,
                    REQUEST,
                    "Response",
                    &("not an unsigned response code", Results::new()),
                )
                .await
                .expect("emit malformed response"),
            ResponseMode::Late | ResponseMode::Rogue => {
                if matches!(self.mode, ResponseMode::Rogue) {
                    respond(&self.rogue, &sender, &path, 1).await;
                }
                let connection = connection.clone();
                let path = path.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    respond(&connection, &sender, &path, 0).await;
                });
            }
            ResponseMode::ServiceLoss => {
                let connection = connection.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    connection
                        .release_name(NAME)
                        .await
                        .expect("release portal name");
                });
            }
            ResponseMode::Replacement => {
                let replacement = self.rogue.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    replacement
                        .request_name_with_flags(
                            NAME,
                            zbus::fdo::RequestNameFlags::ReplaceExisting.into(),
                        )
                        .await
                        .expect("replace portal owner");
                });
            }
            ResponseMode::Silent => {}
        }
        if matches!(self.mode, ResponseMode::WrongPath) {
            OwnedObjectPath::try_from("/org/freedesktop/portal/desktop/request/wrong/path")
                .expect("wrong path")
        } else {
            path
        }
    }

    async fn bad_reply(&self, _parent: &str, _title: &str, _options: Results) -> String {
        "reply must be an object path".into()
    }
}

struct Usb {
    mode: UsbMode,
    calls: Arc<Mutex<Calls>>,
}

fn fd_information(fd: OwnedFd) -> Results {
    let value = Value::from(Fd::from(fd));
    Results::from([
        ("success".into(), true.into()),
        (
            "fd".into(),
            OwnedValue::try_from(value).expect("own FD value"),
        ),
    ])
}

#[zbus::interface(name = "org.freedesktop.portal.Usb")]
impl Usb {
    async fn enumerate_devices(&self, _options: Results) -> Vec<(String, Results)> {
        let properties = Results::from([
            ("ID_VENDOR_ID".into(), Str::from("5345").into()),
            ("ID_MODEL_ID".into(), Str::from("1234").into()),
        ]);
        vec![(
            "scope-a".into(),
            Results::from([
                ("readable".into(), true.into()),
                ("writable".into(), true.into()),
                ("properties".into(), properties.into()),
            ]),
        )]
    }

    async fn acquire_devices(
        &self,
        _parent: &str,
        devices: Vec<(String, Results)>,
        mut options: Results,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> OwnedObjectPath {
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].0, "scope-a");
        assert!(
            bool::try_from(
                devices[0].1["writable"]
                    .try_clone()
                    .expect("clone writable")
            )
            .expect("writable bool")
        );
        let path = request_path(&header, &mut options);
        respond(
            connection,
            header.sender().expect("sender").as_str(),
            &path,
            if matches!(self.mode, UsbMode::Cancel) {
                1
            } else {
                0
            },
        )
        .await;
        path
    }

    async fn finish_acquire_devices(
        &self,
        _path: OwnedObjectPath,
        _options: Results,
    ) -> (Vec<(String, Results)>, bool) {
        self.calls.lock().expect("calls lock").finish += 1;
        let fd = || {
            fd_information(
                fs::File::open("/dev/null")
                    .expect("open harmless FD")
                    .into(),
            )
        };
        let results = match self.mode {
            UsbMode::Valid | UsbMode::Cancel => vec![("scope-a".into(), fd())],
            UsbMode::WrongId => vec![("scope-b".into(), fd())],
            UsbMode::Duplicate => vec![("scope-a".into(), fd()), ("scope-a".into(), fd())],
            UsbMode::MissingFd => vec![(
                "scope-a".into(),
                Results::from([("success".into(), true.into())]),
            )],
            UsbMode::WrongFdType => vec![(
                "scope-a".into(),
                Results::from([("success".into(), true.into()), ("fd".into(), true.into())]),
            )],
            UsbMode::MissingDevice => vec![],
            UsbMode::Denied => vec![(
                "scope-a".into(),
                Results::from([
                    ("success".into(), false.into()),
                    ("error".into(), Str::from("access denied").into()),
                ]),
            )],
        };
        (results, true)
    }

    async fn release_devices(&self, devices: Vec<String>, _options: Results) {
        self.calls
            .lock()
            .expect("calls lock")
            .release
            .extend(devices);
    }
}

struct Fixture {
    portal: Portal,
    server: Connection,
    _rogue: Connection,
    runtime: Arc<Runtime>,
    calls: Arc<Mutex<Calls>>,
    bus: PrivateBus,
}

impl Fixture {
    fn new(mode: ResponseMode, usb: Option<UsbMode>) -> Self {
        let bus = PrivateBus::start();
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("test runtime"),
        );
        let calls = Arc::new(Mutex::new(Calls::default()));
        let (server, rogue, client) = runtime.block_on(async {
            let rogue = Builder::address(bus.address.as_str())
                .expect("rogue address")
                .build()
                .await
                .expect("rogue connection");
            let mut builder = Builder::address(bus.address.as_str())
                .expect("server address")
                .name(NAME)
                .expect("portal name")
                .allow_name_replacements(true)
                .serve_at(
                    PATH,
                    Chooser {
                        mode,
                        rogue: rogue.clone(),
                        calls: Arc::clone(&calls),
                    },
                )
                .expect("chooser interface");
            if let Some(mode) = usb {
                builder = builder
                    .serve_at(
                        PATH,
                        Usb {
                            mode,
                            calls: Arc::clone(&calls),
                        },
                    )
                    .expect("USB interface");
            }
            let server = builder.build().await.expect("server connection");
            let client = Builder::address(bus.address.as_str())
                .expect("client address")
                .build()
                .await
                .expect("client connection");
            (server, rogue, client)
        });
        let portal =
            Portal::with_connection(client, Arc::clone(&runtime)).expect("portal on private bus");
        Self {
            portal,
            server,
            _rogue: rogue,
            runtime,
            calls,
            bus,
        }
    }

    fn request(&self, method: &str) -> Result<vdsportal::portal::PortalResponse, PortalError> {
        let token = self.portal.token();
        let options = Results::from([("handle_token".into(), Str::from(token.clone()).into())]);
        self.runtime.block_on(async {
            tokio::time::timeout(
                Duration::from_secs(3),
                self.portal
                    .request_async(CHOOSER, method, &("", "test", options), &token),
            )
            .await
            .expect("portal request must resolve promptly")
        })
    }
}

#[test]
fn subscribes_before_call_and_keeps_one_connection() {
    let fixture = Fixture::new(ResponseMode::Early, None);
    fixture.request("SaveFile").expect("early response");
    fixture.request("SaveFile").expect("second early response");
    let calls = fixture.calls.lock().expect("calls lock");
    assert_eq!(calls.senders.len(), 2);
    assert_eq!(
        calls.senders[0], calls.senders[1],
        "requests must share the grant-owning connection"
    );
}

#[test]
fn accepts_response_after_method_reply() {
    Fixture::new(ResponseMode::Late, None)
        .request("SaveFile")
        .expect("late response");
}

#[test]
fn rejects_unrelated_directed_response_sender() {
    Fixture::new(ResponseMode::Rogue, None)
        .request("SaveFile")
        .expect("ignore rogue cancellation and accept portal response");
}

#[test]
fn cancellation_remains_distinct_from_failure() {
    assert!(matches!(
        Fixture::new(ResponseMode::Cancel, None).request("SaveFile"),
        Err(PortalError::Cancelled)
    ));
    assert!(matches!(
        Fixture::new(ResponseMode::Failed, None).request("SaveFile"),
        Err(PortalError::Failed(_))
    ));
}

#[test]
fn validates_returned_request_path_even_with_early_success() {
    let result = Fixture::new(ResponseMode::WrongPath, None).request("SaveFile");
    assert!(
        matches!(result, Err(PortalError::Failed(message)) if message.contains("unexpected request path"))
    );
}

#[test]
fn rejects_malformed_method_reply_and_response() {
    let malformed = Fixture::new(ResponseMode::Malformed, None).request("SaveFile");
    assert!(
        matches!(malformed, Err(PortalError::Failed(message)) if message.contains("malformed response"))
    );
    let reply = Fixture::new(ResponseMode::Early, None).request("BadReply");
    assert!(
        matches!(reply, Err(PortalError::Failed(message)) if message.contains("Malformed portal request reply"))
    );
}

#[test]
fn service_name_loss_resolves_pending_request() {
    let result = Fixture::new(ResponseMode::ServiceLoss, None).request("SaveFile");
    assert!(matches!(result, Err(PortalError::Failed(message)) if message.contains("stopped")));
}

#[test]
fn bus_loss_resolves_pending_request() {
    let mut fixture = Fixture::new(ResponseMode::Silent, None);
    let token = fixture.portal.token();
    let options = Results::from([("handle_token".into(), Str::from(token.clone()).into())]);
    fixture.runtime.block_on(async {
        let body = ("", "test", options);
        let request = fixture
            .portal
            .request_async(CHOOSER, "SaveFile", &body, &token);
        tokio::pin!(request);
        tokio::select! {
            result = &mut request => panic!("silent request unexpectedly resolved: {result:?}"),
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
        fixture.bus.child.kill().expect("disconnect private bus");
        let result = tokio::time::timeout(Duration::from_secs(3), &mut request)
            .await
            .expect("bus loss must resolve request");
        assert!(matches!(result, Err(PortalError::Failed(_))));
    });
}

#[test]
fn service_connection_loss_resolves_pending_request() {
    let fixture = Fixture::new(ResponseMode::Silent, None);
    let portal = fixture.portal.clone();
    let server = fixture.server.clone();
    let token = portal.token();
    let options = Results::from([("handle_token".into(), Str::from(token.clone()).into())]);
    fixture.runtime.block_on(async {
        let body = ("", "test", options);
        let request = portal.request_async(CHOOSER, "SaveFile", &body, &token);
        tokio::pin!(request);
        tokio::select! {
            result = &mut request => panic!("silent request unexpectedly resolved: {result:?}"),
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
        server.close().await.expect("close service connection");
        let result = tokio::time::timeout(Duration::from_secs(3), &mut request)
            .await
            .expect("service loss must resolve request");
        assert!(matches!(result, Err(PortalError::Failed(_))));
    });
}

#[test]
fn usb_interface_absence_fails_without_raw_device_fallback() {
    let fixture = Fixture::new(ResponseMode::Early, None);
    assert!(matches!(
        fixture.portal.enumerate_usb(),
        Err(PortalError::Failed(_))
    ));
    assert!(matches!(
        fixture.portal.acquire_usb("scope-a"),
        Err(PortalError::Failed(_))
    ));
}

#[test]
fn usb_enumeration_is_typed_and_acquisition_returns_owned_fd() {
    let fixture = Fixture::new(ResponseMode::Early, Some(UsbMode::Valid));
    let devices = fixture.portal.enumerate_usb().expect("USB enumeration");
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].id, "scope-a");
    assert!(devices[0].readable && devices[0].writable);
    assert_eq!(devices[0].vendor_id.as_deref(), Some("5345"));
    assert_eq!(devices[0].product_id.as_deref(), Some("1234"));
    let fd = fixture
        .portal
        .acquire_usb("scope-a")
        .expect("valid portal FD");
    let file = fs::File::from(fd);
    file.metadata().expect("received FD is usable");
    drop(file);
    fixture
        .portal
        .release_usb("scope-a")
        .expect("release grant");
    assert_eq!(
        fixture.calls.lock().expect("calls lock").release,
        ["scope-a"]
    );
}

#[test]
fn malformed_usb_results_release_only_the_requested_id() {
    for mode in [
        UsbMode::WrongId,
        UsbMode::Duplicate,
        UsbMode::MissingFd,
        UsbMode::WrongFdType,
        UsbMode::MissingDevice,
        UsbMode::Denied,
    ] {
        let fixture = Fixture::new(ResponseMode::Early, Some(mode));
        assert!(matches!(
            fixture.portal.acquire_usb("scope-a"),
            Err(PortalError::Failed(_))
        ));
        assert_eq!(
            fixture.calls.lock().expect("calls lock").release,
            ["scope-a"],
            "a malformed result must not revoke scope-b's grant"
        );
    }
}

#[test]
fn usb_cancellation_never_finishes_or_releases_unacquired_devices() {
    let fixture = Fixture::new(ResponseMode::Early, Some(UsbMode::Cancel));
    assert!(matches!(
        fixture.portal.acquire_usb("scope-a"),
        Err(PortalError::Cancelled)
    ));
    let calls = fixture.calls.lock().expect("calls lock");
    assert_eq!(calls.finish, 0);
    assert!(calls.release.is_empty());
}

#[test]
fn direct_service_owner_replacement_resolves_pending_request() {
    let result = Fixture::new(ResponseMode::Replacement, None).request("SaveFile");
    assert!(
        matches!(result, Err(PortalError::Failed(message)) if message.contains("stopped") || message.contains("changed"))
    );
}
