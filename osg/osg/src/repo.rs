use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;

use failure::{err_msg, format_err, Fallible};
use futures::{future, prelude::*};
use log::*;
use serde_derive::{Deserialize, Serialize};

use crate::model::*;
use keyvault::PublicKey as KeyVaultPublicKey;

pub trait ProfileRepository {
    fn get(&self, id: &ProfileId) -> AsyncFallible<ProfileData>;
    fn set(&mut self, profile: ProfileData) -> AsyncFallible<()>;
    // clear up links and attributes to leave an empty tombstone in place of the profile.
    fn clear(&mut self, key: &PublicKey) -> AsyncFallible<()>;
}

pub trait LocalProfileRepository: ProfileRepository {
    // NOTE similar to set() but without version check, must be able to revert to a previous version
    fn restore(&mut self, id: ProfileId, profile: ProfileData) -> Fallible<()>;
}

pub trait ProfileExplorer {
    fn followers(&self, id: &ProfileId) -> Fallible<Vec<Link>>;
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FileProfileRepository {
    profiles: HashMap<ProfileId, ProfileData>,
    #[serde(skip)]
    filename: PathBuf,
}

impl FileProfileRepository {
    pub fn create(filename: &PathBuf) -> Fallible<Self> {
        let this = Self { profiles: Default::default(), filename: filename.to_owned() };
        this.save()?;
        Ok(this)
    }

    fn save(&self) -> Fallible<()> {
        debug!("Saving profile repository to {:?}", self.filename);
        if let Some(repo_dir) = self.filename.parent() {
            debug!("Recursively Creating directory {:?}", repo_dir);
            std::fs::create_dir_all(repo_dir)?;
        }

        let repo_file = File::create(&self.filename)?;
        bincode::serialize_into(repo_file, self)?;
        Ok(())
    }

    pub fn load(filename: &PathBuf) -> Fallible<Self> {
        debug!("Loading profile repository from {:?}", filename);
        let repo_file = File::open(filename)?;
        let mut repo: Self = bincode::deserialize_from(repo_file)?;
        repo.filename = filename.to_owned();
        Ok(repo)
    }

    fn put(&mut self, id: ProfileId, profile: ProfileData) -> Fallible<()> {
        self.profiles.insert(id, profile);
        self.save()?;
        Ok(())
    }

    fn delete(&mut self, key: &PublicKey) -> Fallible<()> {
        let id = key.key_id();
        let profile =
            self.profiles.get(&id).ok_or_else(|| format_err!("Profile not found: {}", key))?;
        self.put(id, ProfileData::tombstone(key, profile.version()))
    }
}

impl ProfileRepository for FileProfileRepository {
    fn get(&self, id: &ProfileId) -> AsyncFallible<ProfileData> {
        // TODO we probably should also have some nicely typed errors here
        let res = self
            .profiles
            .get(id)
            .map(|prof_ref| prof_ref.to_owned())
            .ok_or_else(|| format_err!("Profile not found: {}", id));
        Box::new(res.into_future())
    }

    fn set(&mut self, profile: ProfileData) -> AsyncFallible<()> {
        if let Some(old_profile) = self.profiles.get(&profile.id()) {
            if old_profile.version() > profile.version()
                || (old_profile.version() == profile.version() && *old_profile != profile)
            {
                return Box::new(future::err(err_msg("Version must increase on profile change")));
            }
        }

        let res = self.put(profile.id(), profile);
        Box::new(res.into_future())
    }

    fn clear(&mut self, key: &PublicKey) -> AsyncFallible<()> {
        let res = self.delete(key);
        Box::new(res.into_future())
    }
}

impl LocalProfileRepository for FileProfileRepository {
    fn restore(&mut self, id: ProfileId, profile: ProfileData) -> Fallible<()> {
        self.put(id, profile)
    }
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use super::*;
    use keyvault::PublicKey as KeyVaultPublicKey;

    #[test]
    fn test_local_repository() -> Fallible<()> {
        let tmp_file = std::env::temp_dir().join("local_repo_test.dat");
        let mut repo = FileProfileRepository::create(&tmp_file)?;

        let my_pubkey = PublicKey::from_str("PezAgmjPHe5Qs4VakvXHGnd6NsYjaxt4suMUtf39TayrSfb")?;
        //let my_id = ProfileId::from_str("IezbeWGSY2dqcUBqT8K7R14xr")?;
        let my_id = my_pubkey.key_id();
        let mut my_data = ProfileData::new(&my_pubkey);
        repo.set(my_data.clone()).wait()?;

        let peer_pubkey = PublicKey::from_str("PezFVen3X669xLzsi6N2V91DoiyzHzg1uAgqiT8jZ9nS96Z")?;
        //let peer_id = ProfileId::from_str("Iez25N5WZ1Q6TQpgpyYgiu9gTX")?;
        let peer_id = peer_pubkey.key_id();
        let peer_data = ProfileData::new(&peer_pubkey);
        repo.set(peer_data.clone()).wait()?;

        let mut me = repo.get(&my_id).wait()?;
        let peer = repo.get(&peer_id).wait()?;
        assert_eq!(me, my_data);
        assert_eq!(peer, peer_data);

        let attr_id = "1 2 3".to_owned();
        let attr_val = "one two three".to_owned();
        my_data.set_attribute(attr_id, attr_val);
        let _link = my_data.create_link(&peer_id);
        my_data.increase_version();
        repo.set(my_data.clone()).wait()?;
        me = repo.get(&my_id).wait()?;
        assert_eq!(me, my_data);
        assert_eq!(me.version(), 2);
        assert_eq!(me.attributes().len(), 1);
        assert_eq!(me.links().len(), 1);

        repo.clear(&my_pubkey).wait()?;
        me = repo.get(&my_id).wait()?;
        assert_eq!(me, ProfileData::create(my_pubkey, 3, Default::default(), Default::default()));

        std::fs::remove_file(&tmp_file)?;

        Ok(())
    }
}
