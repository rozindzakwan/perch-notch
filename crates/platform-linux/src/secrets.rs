//! Mail passwords in the freedesktop Secret Service (GNOME Keyring on
//! Cinnamon, KWallet on KDE), over the session bus with zbus. Items carry the
//! attributes `application=perch`, `perch-key=<key>` and show up in "Passwords
//! and Keys" (Seahorse) under the login keyring.
//!
//! Perch never creates keyrings or answers unlock prompts: if the default
//! keyring is locked or missing, it says so and the account shows an error.

use std::collections::HashMap;

use perch_core::error::{Error, Result};
use perch_core::platform::SecretStore;
use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

const SVC: &str = "org.freedesktop.secrets";
const PATH: &str = "/org/freedesktop/secrets";
const SERVICE: &str = "org.freedesktop.Secret.Service";
const COLLECTION: &str = "org.freedesktop.Secret.Collection";
const ITEM: &str = "org.freedesktop.Secret.Item";
const SESSION: &str = "org.freedesktop.Secret.Session";

/// (session, parameters, value, content type) as the spec defines `Secret`.
type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

/// [`SecretStore`] over the Secret Service.
pub struct SecretService {
    conn: Connection,
}

fn err(what: &str, e: impl std::fmt::Display) -> Error {
    Error::backend(format!("keyring {what}: {e}"))
}

impl SecretService {
    /// Connect to the session bus. Fails if there is no session bus; whether
    /// a Secret Service is running is only known at the first call.
    pub fn connect() -> Result<Self> {
        let conn = Connection::session().map_err(|e| err("connect", e))?;
        Ok(Self { conn })
    }

    fn call<B, R>(&self, path: &str, iface: &str, method: &str, body: &B) -> Result<R>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
        R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
    {
        let reply = self
            .conn
            .call_method(Some(SVC), path, Some(iface), method, body)
            .map_err(|e| match e {
                zbus::Error::MethodError(name, _, _)
                    if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" =>
                {
                    Error::unavailable(
                        "keyring",
                        "no Secret Service (GNOME Keyring / KWallet) is running",
                    )
                }
                other => err(method, other),
            })?;
        reply.body().deserialize::<R>().map_err(|e| err(method, e))
    }

    fn open_session(&self) -> Result<OwnedObjectPath> {
        let (_out, path): (OwnedValue, OwnedObjectPath) =
            self.call(PATH, SERVICE, "OpenSession", &("plain", Value::from("")))?;
        Ok(path)
    }

    fn close_session(&self, session: &OwnedObjectPath) {
        let _: Result<()> = self.call(session.as_str(), SESSION, "Close", &());
    }

    fn search(&self, key: &str) -> Result<(Vec<OwnedObjectPath>, Vec<OwnedObjectPath>)> {
        self.call(PATH, SERVICE, "SearchItems", &(attrs(key),))
    }
}

fn attrs(key: &str) -> HashMap<&str, &str> {
    HashMap::from([("application", "perch"), ("perch-key", key)])
}

fn no_prompt(p: &OwnedObjectPath) -> bool {
    p.as_str() == "/"
}

impl SecretStore for SecretService {
    fn get(&self, key: &str) -> Result<Option<String>> {
        let (mut unlocked, locked) = self.search(key)?;
        if unlocked.is_empty() && !locked.is_empty() {
            let (now_unlocked, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) =
                self.call(PATH, SERVICE, "Unlock", &(locked,))?;
            if !no_prompt(&prompt) {
                return Err(Error::unavailable(
                    "keyring",
                    "the keyring is locked; unlock it and restart Perch",
                ));
            }
            unlocked = now_unlocked;
        }
        let Some(item) = unlocked.into_iter().next() else {
            return Ok(None);
        };
        let session = self.open_session()?;
        let secrets: Result<HashMap<OwnedObjectPath, Secret>> =
            self.call(PATH, SERVICE, "GetSecrets", &(vec![item], &session));
        self.close_session(&session);
        let value = secrets?.into_values().next().map(|s| s.2);
        match value {
            None => Ok(None),
            Some(v) => String::from_utf8(v)
                .map(Some)
                .map_err(|_| Error::backend("stored secret is not UTF-8")),
        }
    }

    fn set(&self, key: &str, label: &str, secret: &str) -> Result<()> {
        let collection: OwnedObjectPath = self.call(PATH, SERVICE, "ReadAlias", &("default",))?;
        if no_prompt(&collection) {
            return Err(Error::unavailable("keyring", "there is no default keyring"));
        }
        let session = self.open_session()?;
        let mut props: HashMap<&str, Value<'_>> = HashMap::new();
        props.insert("org.freedesktop.Secret.Item.Label", Value::from(label));
        props.insert(
            "org.freedesktop.Secret.Item.Attributes",
            Value::from(attrs(key)),
        );
        let secret_value = (
            ObjectPath::from(&session),
            Vec::<u8>::new(),
            secret.as_bytes().to_vec(),
            "text/plain; charset=utf8",
        );
        let created: Result<(OwnedObjectPath, OwnedObjectPath)> = self.call(
            collection.as_str(),
            COLLECTION,
            "CreateItem",
            &(props, secret_value, true),
        );
        self.close_session(&session);
        let (_item, prompt) = created?;
        if !no_prompt(&prompt) {
            return Err(Error::unavailable(
                "keyring",
                "the keyring is locked; unlock it and try again",
            ));
        }
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<()> {
        let (unlocked, locked) = self.search(key)?;
        for item in unlocked.into_iter().chain(locked) {
            let _prompt: OwnedObjectPath = self.call(item.as_str(), ITEM, "Delete", &())?;
        }
        Ok(())
    }
}
