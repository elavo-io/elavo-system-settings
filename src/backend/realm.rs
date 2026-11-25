//! Active Directory and FreeIPA membership through realmd
//! (`org.freedesktop.realmd`). realmd finds the domain, joins and leaves
//! it, and keeps sssd's login policy; it asks polkit for an administrator
//! itself. The directory administrator's password goes to realmd as a D-Bus
//! credential and nowhere else.

use std::collections::HashMap;

use gtk::prelude::*;
use gtk::{gio, glib};

use super::dbus::{self, prop, prop_str};

const REALMD: &str = "org.freedesktop.realmd";
const REALMD_PATH: &str = "/org/freedesktop/realmd";
const PROVIDER: &str = "org.freedesktop.realmd.Provider";
const SERVICE: &str = "org.freedesktop.realmd.Service";
const REALM: &str = "org.freedesktop.realmd.Realm";
const KERBEROS: &str = "org.freedesktop.realmd.Kerberos";
const MEMBERSHIP: &str = "org.freedesktop.realmd.KerberosMembership";
/// Joining FreeIPA installs its client and can take minutes; the polkit
/// prompt waits for the person as well.
const JOIN_TIMEOUT_MS: i32 = 20 * 60 * 1000;
/// One polkit action that covers both joining and the login-policy
/// change that follows on Active Directory. Authorizing it implies
/// `org.freedesktop.realmd.configure-realm` and
/// `org.freedesktop.realmd.login-policy`, so the person is asked once.
pub const JOIN_ACTION: &str = "io.elavo.SystemSettings.domain-join";

/// Ask for permission to join (and, on Active Directory, to set who
/// can sign in afterwards). `Ok(false)` means the prompt was cancelled.
pub async fn authorize_join() -> Result<bool, String> {
    dbus::authorize(JOIN_ACTION).await
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    ActiveDirectory,
    FreeIpa,
    /// A Kerberos realm realmd can see but Elavo does not set up.
    Other,
}

impl Kind {
    fn from_server_software(software: &str) -> Kind {
        match software {
            "active-directory" => Kind::ActiveDirectory,
            "ipa" | "freeipa" => Kind::FreeIpa,
            _ => Kind::Other,
        }
    }
}

/// realmd's login policy for a joined realm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// Everyone in the directory.
    Everyone,
    /// Only `PermittedLogins` and `PermittedGroups`.
    Permitted,
    /// No directory account.
    Nobody,
    /// realmd does not manage it (FreeIPA: HBAC on the server).
    Unknown,
}

impl Policy {
    fn parse(value: &str) -> Policy {
        match value {
            "allow-realm-logins" | "allow-any-login" => Policy::Everyone,
            "allow-permitted-logins" => Policy::Permitted,
            "deny-any-login" => Policy::Nobody,
            _ => Policy::Unknown,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Realm {
    pub path: String,
    /// The domain, as realmd names the realm: `ad.example.com`.
    pub name: String,
    /// The Kerberos realm: `AD.EXAMPLE.COM`.
    pub realm_name: String,
    pub kind: Kind,
    pub configured: bool,
    pub login_formats: Vec<String>,
    pub policy: Policy,
    pub permitted_groups: Vec<String>,
    pub permitted_logins: Vec<String>,
    pub suggested_admin: String,
}

impl Realm {
    pub fn kind_name(&self) -> String {
        use crate::i18n::tr;
        match self.kind {
            Kind::ActiveDirectory => tr("Active Directory"),
            Kind::FreeIpa => tr("FreeIPA"),
            Kind::Other => tr("Kerberos realm"),
        }
    }

    /// The administrator group a new directory usually has.
    pub fn usual_admin_group(&self) -> &'static str {
        match self.kind {
            Kind::FreeIpa => "admins",
            _ => "Domain Admins",
        }
    }
}

fn details(value: Option<&glib::Variant>) -> HashMap<String, String> {
    value
        .and_then(|v| v.get::<Vec<(String, String)>>())
        .unwrap_or_default()
        .into_iter()
        .collect()
}

fn strings(props: &dbus::Props, key: &str) -> Vec<String> {
    props.get(key).map(dbus::strings).unwrap_or_default()
}

async fn realm(conn: &gio::DBusConnection, path: &str) -> Option<Realm> {
    let r = dbus::get_all(conn, REALMD, path, REALM).await;
    if r.is_empty() {
        return None;
    }
    let k = dbus::get_all(conn, REALMD, path, KERBEROS).await;
    let m = dbus::get_all(conn, REALMD, path, MEMBERSHIP).await;
    let details = details(r.get("Details"));
    let name = prop_str(&r, "Name");
    let mut realm_name = prop_str(&k, "RealmName");
    if realm_name.is_empty() {
        realm_name = name.to_uppercase();
    }
    Some(Realm {
        path: path.to_owned(),
        name,
        realm_name,
        kind: Kind::from_server_software(
            details
                .get("server-software")
                .map(String::as_str)
                .unwrap_or(""),
        ),
        configured: !prop_str(&r, "Configured").is_empty(),
        login_formats: strings(&r, "LoginFormats"),
        policy: Policy::parse(&prop_str(&r, "LoginPolicy")),
        permitted_groups: strings(&r, "PermittedGroups"),
        permitted_logins: strings(&r, "PermittedLogins"),
        suggested_admin: prop::<String>(&m, "SuggestedAdministrator").unwrap_or_default(),
    })
}

/// The realms this computer is joined to. realmd starts on demand.
pub async fn joined(conn: &gio::DBusConnection) -> Result<Vec<Realm>, glib::Error> {
    let paths = dbus::call(
        conn,
        REALMD,
        REALMD_PATH,
        "org.freedesktop.DBus.Properties",
        "Get",
        Some((PROVIDER, "Realms").to_variant()),
    )
    .await?
    .child_value(0)
    .as_variant()
    .map(|v| dbus::strings(&v))
    .unwrap_or_default();
    let mut out = Vec::new();
    for path in paths {
        if let Some(r) = realm(conn, &path).await {
            if r.configured {
                out.push(r);
            }
        }
    }
    Ok(out)
}

fn options(pairs: &[(&str, glib::Variant)]) -> glib::Variant {
    let map: HashMap<String, glib::Variant> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect();
    map.to_variant()
}

/// Ask realmd what `domain` is. `None` when no AD or FreeIPA domain answers.
/// Discovery is cancelled through `cancel` when the name changes again.
pub async fn discover(
    conn: &gio::DBusConnection,
    domain: &str,
    cancel: &gio::Cancellable,
) -> Result<Option<Realm>, glib::Error> {
    let args = glib::Variant::tuple_from_iter([
        domain.to_variant(),
        options(&[("client-software", "sssd".to_variant())]),
    ]);
    let reply = conn
        .call_future(
            Some(REALMD),
            REALMD_PATH,
            PROVIDER,
            "Discover",
            Some(&args),
            None,
            gio::DBusCallFlags::NONE,
            2 * 60 * 1000,
        )
        .await?;
    if cancel.is_cancelled() {
        return Err(glib::Error::new(gio::IOErrorEnum::Cancelled, "cancelled"));
    }
    let paths = dbus::strings(&reply.child_value(1));
    for path in paths {
        if let Some(r) = realm(conn, &path).await {
            if r.kind != Kind::Other {
                return Ok(Some(r));
            }
        }
    }
    Ok(None)
}

fn password_credential(user: &str, password: &str) -> glib::Variant {
    glib::Variant::tuple_from_iter([
        "password".to_variant(),
        "administrator".to_variant(),
        (user, password).to_variant().to_variant(),
    ])
}

/// A new id for realmd's `operation` option, which ties its Diagnostics
/// lines and Cancel to one call.
pub fn operation_id() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(1);
    format!(
        "elavo-settings-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

async fn long_call(
    conn: &gio::DBusConnection,
    path: &str,
    iface: &str,
    method: &str,
    args: glib::Variant,
) -> Result<(), glib::Error> {
    conn.call_future(
        Some(REALMD),
        path,
        iface,
        method,
        Some(&args),
        None,
        gio::DBusCallFlags::ALLOW_INTERACTIVE_AUTHORIZATION,
        JOIN_TIMEOUT_MS,
    )
    .await
    .map(|_| ())
}

/// Join `realm` with a directory administrator's name and password.
/// `ou` is the Active Directory organizational unit for the computer.
pub async fn join(
    conn: &gio::DBusConnection,
    realm: &Realm,
    user: &str,
    password: &str,
    ou: &str,
    operation: &str,
) -> Result<(), glib::Error> {
    let mut opts = vec![("operation", operation.to_variant())];
    if !ou.is_empty() && realm.kind == Kind::ActiveDirectory {
        opts.push(("computer-ou", ou.to_variant()));
    }
    let args =
        glib::Variant::tuple_from_iter([password_credential(user, password), options(&opts)]);
    long_call(conn, &realm.path, MEMBERSHIP, "Join", args).await
}

/// Leave `realm`. With an administrator's name and password the
/// computer's account in the directory is removed too; without, only this
/// computer forgets the domain.
pub async fn leave(
    conn: &gio::DBusConnection,
    realm: &Realm,
    admin: Option<(&str, &str)>,
    operation: &str,
) -> Result<(), glib::Error> {
    let opts = options(&[("operation", operation.to_variant())]);
    match admin {
        Some((user, password)) => {
            let args = glib::Variant::tuple_from_iter([password_credential(user, password), opts]);
            long_call(conn, &realm.path, MEMBERSHIP, "Leave", args).await
        }
        None => {
            let args = glib::Variant::tuple_from_iter([opts]);
            long_call(conn, &realm.path, REALM, "Deconfigure", args).await
        }
    }
}

/// Who may sign in, for realms whose login policy realmd keeps (Active
/// Directory). `add` and `remove` are group names.
pub async fn set_policy(
    conn: &gio::DBusConnection,
    realm: &Realm,
    policy: Policy,
    add: &[String],
    remove: &[String],
) -> Result<(), glib::Error> {
    let name = match policy {
        Policy::Everyone => "allow-realm-logins",
        Policy::Permitted => "allow-permitted-logins",
        Policy::Nobody => "deny-any-login",
        Policy::Unknown => return Ok(()),
    };
    let args = glib::Variant::tuple_from_iter([
        name.to_variant(),
        add.to_variant(),
        remove.to_variant(),
        options(&[
            ("groups", true.to_variant()),
            ("operation", operation_id().to_variant()),
        ]),
    ]);
    long_call(conn, &realm.path, REALM, "ChangeLoginPolicy", args).await
}

/// Stop a running Join or Leave.
pub fn cancel(conn: &gio::DBusConnection, operation: &str) {
    let conn = conn.clone();
    let operation = operation.to_owned();
    glib::spawn_future_local(async move {
        let _ = dbus::call(
            &conn,
            REALMD,
            REALMD_PATH,
            SERVICE,
            "Cancel",
            Some((operation.as_str(),).to_variant()),
        )
        .await;
    });
}

/// realmd's progress lines for `operation`, as it prints them.
pub fn on_diagnostics(
    conn: &gio::DBusConnection,
    operation: &str,
    f: impl Fn(&str) + 'static,
) -> gio::SignalSubscription {
    let operation = operation.to_owned();
    conn.subscribe_to_signal(
        Some(REALMD),
        Some(SERVICE),
        Some("Diagnostics"),
        Some(REALMD_PATH),
        None,
        gio::DBusSignalFlags::NONE,
        move |signal| {
            let params = signal.parameters;
            if params.child_value(1).str() == Some(operation.as_str()) {
                if let Some(text) = params.child_value(0).str() {
                    for line in text.lines() {
                        f(line);
                    }
                }
            }
        },
    )
}

/// What to tell the person when realmd refused or failed.
pub fn error_text(error: &glib::Error) -> String {
    use crate::i18n::tr;
    let remote = gio::DBusError::remote_error(error).map(|s| s.to_string());
    let message = crate::widgets::error_text(error);
    match remote.as_deref() {
        Some("org.freedesktop.realmd.Error.AuthenticationFailed") => {
            tr("The directory administrator's name or password was not accepted.")
        }
        Some("org.freedesktop.realmd.Error.NotAuthorized") => {
            tr("You are not allowed to make this change.")
        }
        Some("org.freedesktop.realmd.Error.Cancelled") => tr("Stopped."),
        Some("org.freedesktop.realmd.Error.AlreadyConfigured") => {
            tr("This computer is already joined to a domain.")
        }
        Some("org.freedesktop.realmd.Error.BadHostname") => tr(
            "This computer's name is not accepted by the directory. FreeIPA needs a full name in the domain, such as laptop.example.com.",
        ),
        Some("org.freedesktop.realmd.Error.Busy") => {
            tr("Another join or leave is already running.")
        }
        _ if error.matches(gio::IOErrorEnum::TimedOut) => {
            tr("The directory did not answer in time.")
        }
        _ => message,
    }
}

/// A typed name worth asking realmd about: a DNS name with a dot.
pub fn plausible_domain(text: &str) -> bool {
    let text = text.trim().trim_end_matches('.');
    text.len() <= 253
        && text.contains('.')
        && !text.starts_with('.')
        && text.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

/// `.local` belongs to mDNS, which Elavo resolves with avahi; a directory
/// there resolves unreliably.
pub fn is_mdns_domain(domain: &str) -> bool {
    domain
        .trim_end_matches('.')
        .to_ascii_lowercase()
        .ends_with(".local")
}

/// The directory's servers from DNS, the records realmd and sssd use.
pub async fn servers(domain: &str, kind: Kind) -> Vec<String> {
    let resolver = gio::Resolver::default();
    let zone = match kind {
        Kind::ActiveDirectory => format!("dc._msdcs.{domain}"),
        _ => domain.to_owned(),
    };
    match resolver.lookup_service_future("ldap", "tcp", &zone).await {
        Ok(mut targets) => targets
            .iter_mut()
            .map(|t| t.hostname().to_string())
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Whether `host` answers on the LDAP port within a few seconds.
pub async fn reachable(host: &str) -> bool {
    let client = gio::SocketClient::new();
    client.set_timeout(4);
    client.connect_to_host_future(host, 389).await.is_ok()
}

/// FreeIPA enrolls this computer under its own name, which must be a full
/// name. `None` when it is fine, otherwise the current name.
pub fn hostname_problem(hostname: &str) -> Option<String> {
    let host = hostname.trim().trim_end_matches('.').to_ascii_lowercase();
    let fine = host.contains('.') && !host.starts_with("localhost.");
    (!fine).then(|| hostname.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains_worth_discovering() {
        assert!(plausible_domain("ad.elavo.test"));
        assert!(plausible_domain("corp.example.com."));
        assert!(!plausible_domain("corp"));
        assert!(!plausible_domain("corp..com"));
        assert!(!plausible_domain("bad name.com"));
        assert!(!plausible_domain("-x.com"));
        assert!(!plausible_domain(".com"));
    }

    #[test]
    fn mdns_domains() {
        assert!(is_mdns_domain("corp.local"));
        assert!(is_mdns_domain("CORP.LOCAL."));
        assert!(!is_mdns_domain("corp.localdomain.com"));
    }

    #[test]
    fn policies_and_kinds() {
        assert_eq!(Policy::parse("allow-realm-logins"), Policy::Everyone);
        assert_eq!(Policy::parse("allow-permitted-logins"), Policy::Permitted);
        assert_eq!(Policy::parse("deny-any-login"), Policy::Nobody);
        assert_eq!(Policy::parse(""), Policy::Unknown);
        assert_eq!(
            Kind::from_server_software("active-directory"),
            Kind::ActiveDirectory
        );
        assert_eq!(Kind::from_server_software("ipa"), Kind::FreeIpa);
        assert_eq!(Kind::from_server_software("kerberos"), Kind::Other);
    }

    #[test]
    fn short_hostnames_are_flagged() {
        assert!(hostname_problem("eve").is_some());
        assert!(hostname_problem("localhost.localdomain").is_some());
        assert!(hostname_problem("eve.ipa.elavo.test").is_none());
    }

    #[test]
    fn credentials_have_realmd_shape() {
        let v = password_credential("Administrator", "secret");
        assert_eq!(v.type_().as_str(), "(ssv)");
        let inner = v.child_value(2).as_variant().unwrap();
        assert_eq!(inner.type_().as_str(), "(ss)");
        let opts = options(&[("operation", "x".to_variant())]);
        assert_eq!(opts.type_().as_str(), "a{sv}");
    }
}
