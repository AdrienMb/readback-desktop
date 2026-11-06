//! The `ext_agent_host_id`: one opaque `urn:uuid:` per installation, generated once and reused
//! across restarts, sign-outs and account switches. Not a secret, not an identity.

use std::path::Path;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::errors::{Error, Result};
use super::store::{read_json, write_json_atomic};

const HOST_FILE: &str = "chatgpt-host.json";

#[derive(Serialize, Deserialize)]
struct HostFile {
    version: u32,
    id: String,
}

pub fn load_or_create(dir: &Path) -> Result<String> {
    let path = dir.join(HOST_FILE);
    match read_json::<HostFile>(&path)? {
        Some(file) if file.version == 1 && is_valid(&file.id) => Ok(file.id),
        Some(_) => Err(Error::storage(
            "host_id_invalid",
            "Readback's saved installation ID is damaged. Restore chatgpt-host.json in the app \
             data folder, or remove it to create a new one.",
        )),
        None => {
            let id = format!("urn:uuid:{}", Uuid::new_v4());
            write_json_atomic(
                &path,
                &HostFile {
                    version: 1,
                    id: id.clone(),
                },
            )?;
            Ok(id)
        }
    }
}

pub fn is_valid(id: &str) -> bool {
    id.strip_prefix("urn:uuid:").is_some_and(|rest| {
        Uuid::try_parse(rest)
            .is_ok_and(|uuid| uuid.get_version_num() == 4 && uuid.hyphenated().to_string() == rest)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chatgpt::store::tests::temp_dir;

    #[test]
    fn generated_once_and_reused() {
        let dir = temp_dir();
        let first = load_or_create(&dir).unwrap();
        assert!(is_valid(&first), "{first}");
        assert_eq!(load_or_create(&dir).unwrap(), first);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_malformed_ids() {
        assert!(!is_valid("urn:uuid:not-a-uuid"));
        assert!(!is_valid("6f1c5c1e-8b4e-4a8e-9f1e-0d6c2a3b4c5d"));
        assert!(!is_valid("urn:uuid:6F1C5C1E-8B4E-4A8E-9F1E-0D6C2A3B4C5D"));
        assert!(is_valid("urn:uuid:6f1c5c1e-8b4e-4a8e-9f1e-0d6c2a3b4c5d"));
    }

    #[test]
    fn damaged_file_is_not_silently_replaced() {
        let dir = temp_dir();
        write_json_atomic(
            &dir.join(HOST_FILE),
            &HostFile {
                version: 1,
                id: "bogus".into(),
            },
        )
        .unwrap();
        assert_eq!(load_or_create(&dir).unwrap_err().code, "host_id_invalid");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
