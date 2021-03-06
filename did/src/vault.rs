use std::fs::File;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Weak};

use failure::{bail, ensure, err_msg, format_err, Fallible};
use log::*;
use serde_derive::{Deserialize, Serialize};

use crate::model::*;
use keyvault::{
    ed25519::{Ed25519, EdExtPrivateKey},
    ExtendedPrivateKey, ExtendedPublicKey, KeyDerivationCrypto, PrivateKey as KeyVaultPrivateKey,
    PublicKey as KeyVaultPublicKey, Seed, BIP43_PURPOSE_MERCURY,
};

// TODO exposing public and private keys below should work with MPrivateKey to support
//      any key type, and thus key derivation should be exported to the keyvault::PrivateKey
pub struct HdKeys {
    mercury_xsk: EdExtPrivateKey,
}

impl HdKeys {
    pub fn public_key(&self, idx: i32) -> Fallible<PublicKey> {
        let profile_xsk = self.mercury_xsk.derive_hardened_child(idx)?;
        let key = profile_xsk.neuter().as_public_key();
        Ok(key.into())
    }

    pub fn id(&self, idx: i32) -> Fallible<ProfileId> {
        self.public_key(idx).map(|key| key.key_id())
    }
}

pub struct HdSecrets {
    mercury_xsk: EdExtPrivateKey,
}

impl HdSecrets {
    pub fn private_key(&self, idx: i32) -> Fallible<PrivateKey> {
        let profile_xsk = self.mercury_xsk.derive_hardened_child(idx)?;
        let key = profile_xsk.as_private_key();
        Ok(key.into())
    }
}

pub type ProfileLabel = String;
pub type ProfileMetadata = String;

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, PartialOrd, Serialize)]
pub struct ProfileVaultRecord {
    id: ProfileId,
    label: ProfileLabel,
    metadata: ProfileMetadata,
    // TODO these might be needed as well soon
    // pub bip32_idx: String,
    // pub pubkey: PublicKey,
}

impl ProfileVaultRecord {
    pub fn new(id: ProfileId, label: ProfileLabel, metadata: ProfileMetadata) -> Self {
        Self { id, label, metadata }
    }

    pub fn id(&self) -> ProfileId {
        self.id.to_owned()
    }
    pub fn label(&self) -> ProfileLabel {
        self.label.to_owned()
    }
    pub fn metadata(&self) -> ProfileMetadata {
        self.metadata.to_owned()
    }
}

pub trait ProfileVault {
    fn create_key(&mut self, label: Option<ProfileLabel>) -> Fallible<PublicKey>;
    fn restore_id(&mut self, id: &ProfileId) -> Fallible<()>;

    fn get_active(&self) -> Fallible<Option<ProfileId>>;
    fn set_active(&mut self, id: &ProfileId) -> Fallible<()>;

    // TODO reconsider this API: either label_by_id() and metadata_by_id() or profile()
    //      is redundant and should be removed
    fn label_by_id(&self, id: &ProfileId) -> Fallible<ProfileLabel>;
    fn id_by_label(&self, label: &ProfileLabel) -> Fallible<ProfileId>;
    fn set_label(&mut self, id: ProfileId, label: ProfileLabel) -> Fallible<()>;

    fn metadata_by_id(&self, id: &ProfileId) -> Fallible<ProfileMetadata>;
    fn set_metadata(&mut self, id: ProfileId, data: ProfileMetadata) -> Fallible<()>;

    fn profiles(&self) -> Fallible<Vec<ProfileVaultRecord>>;
    fn profile(&self, id: &ProfileId) -> Fallible<ProfileVaultRecord>;

    fn signer(self: Arc<Self>, profile_id: &ProfileId) -> Fallible<Rc<dyn Signer>>;
    // TODO sign() should be removed and done only via signer(), but that requires using Arc<> or finding another solution
    fn sign(&self, id: &ProfileId, message: &[u8]) -> Fallible<SignedMessage>;
    fn validate(&self, signer_id: Option<ProfileId>, signed_msg: &SignedMessage) -> bool;

    // TODO these probably should not be here on the long run, list() is enough in most cases.
    //      Used only for restoring all profiles of a vault with gap detection.
    fn keys(&self) -> Fallible<HdKeys>;
    fn len(&self) -> usize;

    // TODO this should not be on this interface on the long run.
    //      Used for saving vault state on a change.
    fn save(&self, filename: &PathBuf) -> Fallible<()>;
}

pub const GAP: u32 = 20;

#[derive(Debug, Deserialize, Serialize)]
pub struct HdProfileVault {
    seed: Seed,
    next_idx: i32,
    active_idx: Option<i32>,
    profiles: Vec<ProfileVaultRecord>,
}

impl HdProfileVault {
    pub fn create(seed: Seed) -> Self {
        info!("Initializing new vault");
        Self { seed, next_idx: Default::default(), active_idx: Option::None, profiles: vec![] }
    }

    pub fn load(filename: &PathBuf) -> Fallible<Self> {
        trace!("Loading profile vault from {:?}", filename);
        let vault_file = File::open(filename)?;
        let vault: Self = serde_json::from_reader(&vault_file)?;
        //let vault: Self = bincode::deserialize_from(vault_file)?;
        ensure!(vault.next_idx >= 0, "next_idx cannot be negative");
        if let Some(active) = vault.active_idx {
            ensure!(active >= 0, "active_idx cannot be negative");
            ensure!(active < vault.next_idx, "active_idx cannot exceed last profile index");
        }
        ensure!(vault.next_idx as usize == vault.profiles.len(), "a record must exist for each id");

        use std::{collections::HashSet, iter::FromIterator};
        let unique_labels: HashSet<String> =
            HashSet::from_iter(vault.profiles.iter().map(|rec| rec.label.to_owned()));
        ensure!(vault.profiles.len() == unique_labels.len(), "all labels must be unique");

        Ok(vault)
    }

    fn index_of_id(&self, id: &ProfileId) -> Option<usize> {
        let profiles = self.keys().ok()?;
        for idx in 0..self.next_idx {
            // TODO consider if we should really validate() here or should directly check id equality only
            if profiles.public_key(idx).ok()?.validate_id(id) {
                return Some(idx as usize);
            }
        }
        None
    }

    fn profile_by_id(&self, id: &ProfileId) -> Fallible<&ProfileVaultRecord> {
        self.profiles
            .iter()
            .find(|rec| rec.id == *id)
            .ok_or_else(|| err_msg("profile is not found in vault"))
    }

    fn mut_profile_by_id(&mut self, id: &ProfileId) -> Fallible<&mut ProfileVaultRecord> {
        self.profiles
            .iter_mut()
            .find(|rec| rec.id == *id)
            .ok_or_else(|| err_msg("profile is not found in vault"))
    }

    fn mercury_xsk(&self) -> Fallible<EdExtPrivateKey> {
        let master = Ed25519::master(&self.seed);
        master.derive_hardened_child(BIP43_PURPOSE_MERCURY)
    }

    // TODO this should be protected by some password
    fn secrets(&self) -> Fallible<HdSecrets> {
        Ok(HdSecrets { mercury_xsk: self.mercury_xsk()? })
    }

    fn get_bip32_idx(&self, profile_id: &ProfileId) -> Fallible<i32> {
        let idx = self
            .index_of_id(profile_id)
            .ok_or(format_err!("Profile {} is not found in vault", profile_id))?;
        Ok(idx as i32)
    }

    fn get_public_key(&self, idx: i32) -> Fallible<PublicKey> {
        self.keys()?.public_key(idx)
    }
}

impl ProfileVault for HdProfileVault {
    fn create_key(&mut self, label_opt: Option<ProfileLabel>) -> Fallible<PublicKey> {
        let label = label_opt.unwrap_or(self.profiles.len().to_string());
        ensure!(self.id_by_label(&label).is_err(), "the specified label must be unique");
        ensure!(self.profiles.len() == self.next_idx as usize, "a record must exist for each id");

        let key = self.get_public_key(self.next_idx)?;
        self.profiles.push(ProfileVaultRecord::new(key.key_id(), label, "".to_owned()));

        debug!("Active profile was set to {}", key.key_id());
        self.active_idx = Option::Some(self.next_idx);
        self.next_idx += 1;

        Ok(key)
    }

    fn restore_id(&mut self, id: &ProfileId) -> Fallible<()> {
        if self.index_of_id(id).is_some() {
            trace!("Profile id {} is already present in the vault", id);
            return Ok(());
        }

        trace!(
            "Profile id {} is not contained yet, trying to find it from index {} with {} gap",
            id,
            self.next_idx,
            GAP
        );

        let keys = self.keys()?;
        for idx in self.next_idx..self.next_idx + GAP as i32 {
            if *id == keys.id(idx)? {
                trace!("Profile id {} is found at key index {}", id, idx);
                self.next_idx = idx + 1;
                return Ok(());
            }
        }

        bail!("{} is not owned by this seed", id);
    }

    fn get_active(&self) -> Fallible<Option<ProfileId>> {
        if let Some(idx) = self.active_idx {
            Ok(Option::Some(self.keys()?.id(idx)?))
        } else {
            Ok(Option::None)
        }
    }

    fn set_active(&mut self, id: &ProfileId) -> Fallible<()> {
        if let Some(idx) = self.index_of_id(id) {
            self.active_idx = Option::Some(idx as i32);
            Ok(())
        } else {
            bail!("Profile Id '{}' not found", id)
        }
    }

    fn label_by_id(&self, id: &ProfileId) -> Fallible<ProfileLabel> {
        Ok(self.profile_by_id(id)?.label.to_owned())
    }

    fn id_by_label(&self, label: &ProfileLabel) -> Fallible<ProfileId> {
        // TODO this currently scans all records which might turn out to be too slow.
        //      In such a case, we should use a dedicated index (i.e. Map<label,id>) here
        self.profiles
            .iter()
            .filter_map(|rec| if rec.label == *label { Some(rec.id.to_owned()) } else { None })
            .nth(0)
            .ok_or_else(|| err_msg("label is not found in vault"))
    }

    fn set_label(&mut self, id: ProfileId, label: ProfileLabel) -> Fallible<()> {
        self.mut_profile_by_id(&id)?.label = label;
        Ok(())
    }

    fn metadata_by_id(&self, id: &ProfileId) -> Fallible<ProfileMetadata> {
        Ok(self.profile_by_id(id)?.metadata.to_owned())
    }

    fn set_metadata(&mut self, id: ProfileId, data: ProfileMetadata) -> Fallible<()> {
        self.mut_profile_by_id(&id)?.metadata = data;
        Ok(())
    }

    fn profiles(&self) -> Fallible<Vec<ProfileVaultRecord>> {
        Ok(self.profiles.clone())
    }

    fn profile(&self, id: &ProfileId) -> Fallible<ProfileVaultRecord> {
        Ok(self.profile_by_id(id)?.to_owned())
    }

    fn signer(self: Arc<Self>, profile_id: &ProfileId) -> Fallible<Rc<dyn Signer>> {
        let idx = Self::get_bip32_idx(self.as_ref(), profile_id)?;
        let public_key = self.get_public_key(idx)?;
        Ok(Rc::new(VaultSigner {
            vault: Arc::downgrade(&self),
            profile_id: profile_id.to_owned(),
            idx,
            public_key,
        }))
    }

    fn sign(&self, id: &ProfileId, message: &[u8]) -> Fallible<SignedMessage> {
        let idx = self.get_bip32_idx(id)?;
        let private_key = self.secrets()?.private_key(idx)?;
        let signature = private_key.sign(message.as_ref());
        Ok(SignedMessage::new(private_key.public_key(), message.to_owned(), signature))
    }

    fn validate(&self, signer_id: Option<ProfileId>, signed_msg: &SignedMessage) -> bool {
        let id_ok = match signer_id {
            Some(id) => signed_msg.public_key().validate_id(&id),
            None => true,
        };
        id_ok && signed_msg.validate()
    }

    fn keys(&self) -> Fallible<HdKeys> {
        Ok(HdKeys { mercury_xsk: self.mercury_xsk()? })
    }

    fn len(&self) -> usize {
        self.next_idx as usize
    }

    fn save(&self, filename: &PathBuf) -> Fallible<()> {
        debug!("Saving profile vault to store state");
        if let Some(vault_dir) = filename.parent() {
            debug!("Recursively Creating directory {:?}", vault_dir);
            std::fs::create_dir_all(vault_dir)?;
        }

        let vault_file = File::create(filename)?;
        serde_json::to_writer_pretty(&vault_file, self)?;
        //bincode::serialize_into(vault_file, self)?;
        Ok(())
    }
}

pub struct VaultSigner {
    vault: Weak<HdProfileVault>,
    profile_id: ProfileId,
    idx: i32,
    public_key: PublicKey,
}

impl Signer for VaultSigner {
    fn profile_id(&self) -> &ProfileId {
        &self.profile_id
    }

    fn public_key(&self) -> PublicKey {
        self.public_key.to_owned()
    }

    fn sign(&self, data: &[u8]) -> Fallible<Signature> {
        let vault = self.vault.upgrade().ok_or(err_msg("BUG: failed to access ProfileVault"))?;
        let private_key = vault.secrets()?.private_key(self.idx)?;
        Ok(private_key.sign(data))
    }
}
