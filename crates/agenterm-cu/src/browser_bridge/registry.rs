use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};

use super::{ACU_EXTENSION_ID, ACU_NATIVE_HOST_NAME};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ConnectionId(String);

impl ConnectionId {
    /// The caller must supply 32 bytes from its OS CSPRNG. Random generation is
    /// intentionally kept out of this pure model.
    pub fn from_random(random: [u8; 32]) -> Result<Self, RegistryError> {
        if random.iter().all(|byte| *byte == 0) {
            return Err(RegistryError::RandomConnectionIdInvalid);
        }
        let mut encoded = String::with_capacity(64);
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for byte in random {
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        Ok(Self(encoded))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn parse(encoded: &str) -> Result<Self, RegistryError> {
        Self::from_encoded(encoded.to_owned())
    }

    fn from_encoded(encoded: String) -> Result<Self, RegistryError> {
        if encoded.len() != 64
            || encoded
                .bytes()
                .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
            || encoded.bytes().all(|byte| byte == b'0')
        {
            return Err(RegistryError::RandomConnectionIdInvalid);
        }
        Ok(Self(encoded))
    }
}

impl<'de> Deserialize<'de> for ConnectionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        Self::from_encoded(encoded).map_err(|_| {
            serde::de::Error::custom(
                "connection id must be 64 lowercase hex characters and nonzero",
            )
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessIdentity {
    pub pid: u32,
    /// Stable native start identity; a naked PID never owns an entry.
    pub start_identity: String,
}

impl ProcessIdentity {
    fn validate(&self) -> Result<(), RegistryError> {
        if self.pid == 0
            || self.start_identity.is_empty()
            || self.start_identity.len() > 256
            || self.start_identity.chars().any(char::is_control)
        {
            return Err(RegistryError::ProcessIdentityInvalid);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ConnectionEndpoint {
    NativeMessaging {
        native_host: String,
        extension_id: String,
    },
}

impl ConnectionEndpoint {
    fn validate(&self) -> Result<(), RegistryError> {
        match self {
            Self::NativeMessaging {
                native_host,
                extension_id,
            } if native_host == ACU_NATIVE_HOST_NAME && extension_id == ACU_EXTENSION_ID => Ok(()),
            _ => Err(RegistryError::EndpointInvalid),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionEntry {
    pub connection_id: ConnectionId,
    pub process: ProcessIdentity,
    pub endpoint: ConnectionEndpoint,
}

#[derive(Debug)]
pub struct ConnectionRegistry {
    /// Opaque stable identity supplied by the platform's current-user court.
    owner_user_identity: String,
    entries: BTreeMap<ConnectionId, ConnectionEntry>,
}

impl ConnectionRegistry {
    pub fn new(owner_user_identity: String) -> Result<Self, RegistryError> {
        if owner_user_identity.is_empty()
            || owner_user_identity.len() > 256
            || owner_user_identity.chars().any(char::is_control)
        {
            return Err(RegistryError::CurrentUserIdentityInvalid);
        }
        Ok(Self {
            owner_user_identity,
            entries: BTreeMap::new(),
        })
    }

    pub fn owner_user_identity(&self) -> &str {
        &self.owner_user_identity
    }

    pub fn register(
        &mut self,
        process: ProcessIdentity,
        endpoint: ConnectionEndpoint,
        random: [u8; 32],
    ) -> Result<ConnectionEntry, RegistryError> {
        process.validate()?;
        endpoint.validate()?;
        let connection_id = ConnectionId::from_random(random)?;
        if self.entries.contains_key(&connection_id) {
            return Err(RegistryError::ConnectionIdCollision);
        }
        let entry = ConnectionEntry {
            connection_id: connection_id.clone(),
            process,
            endpoint,
        };
        self.entries.insert(connection_id, entry.clone());
        Ok(entry)
    }

    pub fn get(&self, id: &ConnectionId) -> Option<&ConnectionEntry> {
        self.entries.get(id)
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Removes only entries whose exact PID/start identity is proven absent or
    /// changed. Unknown observations remain retained and explicitly reported.
    pub fn cleanup_stale<F>(&mut self, mut exact_identity_is_live: F) -> StaleCleanup
    where
        F: FnMut(&ProcessIdentity) -> Option<bool>,
    {
        let mut removed = Vec::new();
        let mut retained_unknown = Vec::new();
        self.entries
            .retain(|id, entry| match exact_identity_is_live(&entry.process) {
                Some(true) => true,
                Some(false) => {
                    removed.push(id.clone());
                    false
                }
                None => {
                    retained_unknown.push(id.clone());
                    true
                }
            });
        StaleCleanup {
            removed,
            retained_unknown,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StaleCleanup {
    pub removed: Vec<ConnectionId>,
    pub retained_unknown: Vec<ConnectionId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistryError {
    CurrentUserIdentityInvalid,
    RandomConnectionIdInvalid,
    ConnectionIdCollision,
    ProcessIdentityInvalid,
    EndpointInvalid,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, start: &str) -> ProcessIdentity {
        ProcessIdentity {
            pid,
            start_identity: start.into(),
        }
    }
    fn endpoint() -> ConnectionEndpoint {
        ConnectionEndpoint::NativeMessaging {
            native_host: ACU_NATIVE_HOST_NAME.into(),
            extension_id: ACU_EXTENSION_ID.into(),
        }
    }
    fn registry() -> ConnectionRegistry {
        ConnectionRegistry::new("current-user-stable-id".into()).unwrap()
    }

    #[test]
    fn registry_binds_process_endpoint_and_random_connection_identity() {
        let mut registry = registry();
        let entry = registry
            .register(process(42, "start-100"), endpoint(), [0x5a; 32])
            .unwrap();
        assert_eq!(entry.connection_id.as_str().len(), 64);
        assert_eq!(registry.get(&entry.connection_id), Some(&entry));
        assert_eq!(
            registry.register(process(43, "start-101"), endpoint(), [0x5a; 32]),
            Err(RegistryError::ConnectionIdCollision)
        );
        assert_eq!(
            ConnectionId::from_random([0; 32]),
            Err(RegistryError::RandomConnectionIdInvalid)
        );
    }

    #[test]
    fn deserialization_cannot_bypass_connection_id_invariants() {
        let valid = format!("\"{}\"", "ab".repeat(32));
        assert!(serde_json::from_str::<ConnectionId>(&valid).is_ok());
        for invalid in [
            format!("\"{}\"", "00".repeat(32)),
            format!("\"{}\"", "AB".repeat(32)),
            "\"abcd\"".into(),
            format!("\"{}ag\"", "ab".repeat(31)),
        ] {
            assert!(serde_json::from_str::<ConnectionId>(&invalid).is_err());
        }
    }

    #[test]
    fn rejects_naked_process_and_non_chromium_extension_identity() {
        let mut registry = registry();
        assert_eq!(
            registry.register(process(0, "start"), endpoint(), [1; 32]),
            Err(RegistryError::ProcessIdentityInvalid)
        );
        assert_eq!(
            registry.register(process(1, ""), endpoint(), [1; 32]),
            Err(RegistryError::ProcessIdentityInvalid)
        );
        assert_eq!(
            registry.register(
                process(1, "start"),
                ConnectionEndpoint::NativeMessaging {
                    native_host: ACU_NATIVE_HOST_NAME.into(),
                    extension_id: "legacy-mcu".into()
                },
                [1; 32]
            ),
            Err(RegistryError::EndpointInvalid)
        );
    }

    #[test]
    fn registry_requires_an_explicit_bounded_current_user_identity() {
        assert_eq!(
            ConnectionRegistry::new(String::new()).unwrap_err(),
            RegistryError::CurrentUserIdentityInvalid
        );
        let registry = registry();
        assert_eq!(registry.owner_user_identity(), "current-user-stable-id");
    }

    #[test]
    fn stale_cleanup_removes_only_exact_proven_stale_entries() {
        let mut registry = registry();
        let live = registry
            .register(process(1, "a"), endpoint(), [1; 32])
            .unwrap();
        let stale = registry
            .register(process(2, "b"), endpoint(), [2; 32])
            .unwrap();
        let unknown = registry
            .register(process(3, "c"), endpoint(), [3; 32])
            .unwrap();
        let cleanup = registry.cleanup_stale(|identity| match identity.pid {
            1 => Some(identity.start_identity == "a"),
            2 => Some(false),
            _ => None,
        });
        assert_eq!(cleanup.removed, vec![stale.connection_id]);
        assert_eq!(
            cleanup.retained_unknown,
            vec![unknown.connection_id.clone()]
        );
        assert!(registry.get(&live.connection_id).is_some());
        assert!(registry.get(&unknown.connection_id).is_some());
        assert_eq!(registry.len(), 2);
    }
}
