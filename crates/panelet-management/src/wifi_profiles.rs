// SPDX-License-Identifier: MPL-2.0

//! Bounded, validated Wi-Fi credentials for NVS persistence. Never return this
//! encoding through RPC; only `names` is intended for management responses.
use crate::WifiCredentials;
use serde::{Deserialize, Serialize};

pub const WIFI_PROFILE_CAPACITY: usize = 5;
pub const WIFI_PROFILES_MAX_BYTES: usize = 4096;

#[derive(Clone, Default, Debug)]
pub struct WifiProfiles(Vec<WifiCredentials>);
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Encoded {
    version: u8,
    profiles: Vec<WifiCredentials>,
}

impl WifiProfiles {
    pub fn decode(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() > WIFI_PROFILES_MAX_BYTES {
            return Err("wifi profiles too large");
        }
        let value: Encoded = serde_json::from_slice(bytes).map_err(|_| "invalid wifi profiles")?;
        if value.version != 1 || value.profiles.len() > WIFI_PROFILE_CAPACITY {
            return Err("unsupported wifi profiles");
        }
        let mut names = Vec::new();
        for profile in &value.profiles {
            WifiCredentials::new(profile.ssid().into(), profile.password().into())?;
            if names.contains(&profile.ssid()) {
                return Err("duplicate wifi profile");
            }
            names.push(profile.ssid());
        }
        Ok(Self(value.profiles))
    }
    pub fn encode(&self) -> Result<Vec<u8>, &'static str> {
        let bytes = serde_json::to_vec(&Encoded {
            version: 1,
            profiles: self.0.clone(),
        })
        .map_err(|_| "wifi encoding failed")?;
        if bytes.len() > WIFI_PROFILES_MAX_BYTES {
            return Err("wifi profiles too large");
        }
        Ok(bytes)
    }
    pub fn iter(&self) -> impl Iterator<Item = &WifiCredentials> {
        self.0.iter()
    }
    pub fn names(&self) -> Vec<String> {
        self.0.iter().map(|p| p.ssid().into()).collect()
    }
    pub fn can_save(&self, ssid: &str) -> bool {
        self.0.len() < WIFI_PROFILE_CAPACITY || self.0.iter().any(|p| p.ssid() == ssid)
    }
    /// Most recently successful first. A full list never silently evicts a network.
    pub fn remember(&mut self, profile: WifiCredentials) -> Result<(), &'static str> {
        if !self.can_save(profile.ssid()) {
            return Err("wifi profile limit reached; forget a saved network first");
        }
        self.forget(profile.ssid());
        self.0.insert(0, profile);
        Ok(())
    }
    pub fn forget(&mut self, ssid: &str) {
        self.0.retain(|p| p.ssid() != ssid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credentials_roundtrip_replace_prioritize_and_never_silently_evict() {
        let mut profiles = WifiProfiles::default();
        for n in 0..5 {
            profiles
                .remember(
                    WifiCredentials::new(format!("ap{n}"), "test-only-password".into()).unwrap(),
                )
                .unwrap();
        }
        assert!(!profiles.can_save("sixth"));
        assert!(profiles
            .remember(WifiCredentials::new("sixth".into(), "".into()).unwrap())
            .is_err());
        profiles
            .remember(WifiCredentials::new("ap0".into(), "replacement-only".into()).unwrap())
            .unwrap();
        let restored = WifiProfiles::decode(&profiles.encode().unwrap()).unwrap();
        assert_eq!(restored.names(), ["ap0", "ap4", "ap3", "ap2", "ap1"]);
        assert_eq!(
            restored.iter().next().unwrap().password(),
            "replacement-only"
        );
        assert!(!format!("{restored:?}").contains("replacement-only"));
        profiles.forget("ap3");
        assert!(profiles.can_save("sixth"));
        assert_eq!(
            WifiProfiles::decode(&profiles.encode().unwrap())
                .unwrap()
                .names()
                .len(),
            4
        );
    }
    #[test]
    fn corrupted_oversized_and_invalid_credentials_are_rejected() {
        for bytes in [
            br#"{"version":2,"profiles":[]}"#.as_slice(),
            br#"{"version":1,"profiles":[{"ssid":"a","password":"short"}]}"#,
            br#"{"version":1,"profiles":[{"ssid":"a","password":""},{"ssid":"a","password":""}]}"#,
        ] {
            assert!(WifiProfiles::decode(bytes).is_err());
        }
        assert!(WifiProfiles::decode(&vec![b' '; 4097]).is_err());
    }
}
