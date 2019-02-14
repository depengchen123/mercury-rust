use failure::{bail, ensure, Fallible};
use log::*;
use std::sync::{Arc, RwLock};
use structopt::StructOpt;

use morpheus_storage::{AttributeId, AttributeValue};
use prometheus::types::*;
use prometheus::vault::*;

pub struct CommandContext {
    vault: Box<ProfileVault>,
    store: Box<ProfileStore>,
}

impl CommandContext {
    pub fn new(vault: Box<ProfileVault>, store: Box<ProfileStore>) -> Self {
        Self { vault, store }
    }

    pub fn vault(&self) -> &ProfileVault {
        self.vault.as_ref()
    }

    pub fn mut_vault(&mut self) -> &mut ProfileVault {
        self.vault.as_mut()
    }

    pub fn store(&self) -> &ProfileStore {
        self.store.as_ref()
    }

    pub fn mut_store(&mut self) -> &mut ProfileStore {
        self.store.as_mut()
    }
}

#[derive(Debug, StructOpt)]
#[structopt(name = "prometheus", about = "Command line interface of Prometheus")]
pub enum Command {
    #[structopt(name = "status")]
    Status,

    #[structopt(name = "list")]
    /// List profiles or followers
    List(ListCommand),

    /// Show profile details
    #[structopt(name = "show")]
    Show(ShowCommand),

    #[structopt(name = "create")]
    /// Create profile or link
    Create(CreateCommand),

    #[structopt(name = "remove")]
    /// Remove link // TODO (or profile?)
    Remove(RemoveCommand),

    #[structopt(name = "set")]
    /// Set active profile or attribute
    Set(SetCommand),

    #[structopt(name = "clear")]
    /// Clear attribute
    Clear(ClearCommand),
}

fn selected_profile(
    ctx: &CommandContext,
    my_profile_option: Option<ProfileId>,
) -> Fallible<Arc<RwLock<Profile>>> {
    let profile_opt = my_profile_option
        .or(ctx.vault().get_active()?)
        .and_then(|profile_id| ctx.store().get(&profile_id));
    ensure!(
        profile_opt.is_some(),
        "Command option my_profile_id is unspecified and no active default profile was found"
    );
    Ok(profile_opt.unwrap())
}

#[allow(clippy::let_and_return)] // borrow checker releases write guard too early
fn on_profile<F>(ctx: &CommandContext, my_profile: Option<ProfileId>, f: F) -> Fallible<()>
where
    F: FnOnce(&mut Profile) -> Fallible<()>,
{
    let profile_ptr = selected_profile(ctx, my_profile)?;
    let result = match profile_ptr.write() {
        Ok(mut profile) => f(&mut *profile),
        Err(e) => bail!(
            "Implementation error: failed to get write access to selected profile: {}",
            e
        ),
    };
    result
}

impl Command {
    pub fn execute(self, ctx: &mut CommandContext) -> Fallible<()> {
        match self {
            Command::Create(CreateCommand::Link {
                my_profile_id,
                peer_profile_id,
            }) => {
                on_profile(ctx, my_profile_id, |profile| {
                    let link = profile.create_link(&peer_profile_id);
                    info!("Created link to pfofile {:?}", link);
                    Ok(())
                })?;
            }

            Command::Create(CreateCommand::Profile) => {
                let new_profile_id = ctx.mut_vault().create_id()?;
                let created_profile_ptr = ctx.store().create(&new_profile_id)?;
                let created_profile = match created_profile_ptr.read() {
                    Ok(profile) => profile,
                    Err(e) => bail!(
                        "Implementation error: failed to read created profile: {}",
                        e
                    ),
                };
                info!("Created profile with id {}", created_profile.id());
            }

            Command::Clear(ClearCommand::Attribute { my_profile_id, key }) => {
                on_profile(ctx, my_profile_id, |profile| {
                    info!("Clearing attribute: {:?}", key);
                    profile.clear_attribute(&key)?;
                    Ok(())
                })?;
            }

            Command::List(ListCommand::IncomingLinks { my_profile_id }) => {
                on_profile(ctx, my_profile_id, |profile| {
                    let followers = profile.followers()?;
                    info!("Received {} followers", followers.len());
                    for (idx, follower) in followers.iter().enumerate() {
                        info!("  {}: {:?}", idx, follower);
                    }
                    Ok(())
                })?;
            }

            Command::List(ListCommand::Profiles) => {
                // TODO implement listing profiles
            }

            Command::Remove(RemoveCommand::Link {
                my_profile_id,
                peer_profile_id,
            }) => {
                on_profile(ctx, my_profile_id, |profile| {
                    profile.remove_link(&peer_profile_id)?;
                    info!("Removed link from profile {:?}", peer_profile_id);
                    Ok(())
                })?;
            }

            Command::Set(SetCommand::ActiveProfile { my_profile_id }) => {
                ctx.mut_vault().set_active(&my_profile_id)?;
                info!("Active profile was set to {:?}", my_profile_id);
            }

            Command::Set(SetCommand::Attribute {
                my_profile_id,
                key,
                value,
            }) => {
                on_profile(ctx, my_profile_id, |profile| {
                    info!("Setting attribute {} to {}", key, value);
                    profile.set_attribute(&key, &value)?;
                    Ok(())
                })?;
            }

            Command::Show(ShowCommand::Profile { profile_id }) => {
                // TODO display profile
                // NOTE must also work with a profile that is not ours
            }

            Command::Status => {
                // TODO what status to display besides active (default) profile?
            }
        };

        Ok(())
    }
}

#[derive(Debug, StructOpt)]
pub enum ListCommand {
    #[structopt(name = "profiles")]
    /// List profiles
    Profiles,

    #[structopt(name = "followers")]
    /// List followers
    IncomingLinks {
        #[structopt(long = "my_profile_id")]
        /// List public followers of this profile of yours if other than the active one
        my_profile_id: Option<ProfileId>,
    },
}

#[derive(Debug, StructOpt)]
pub enum ShowCommand {
    #[structopt(name = "profile")]
    /// Show profile
    Profile {
        #[structopt(long = "profile_id")]
        /// Profile id to be shown, either yours or remote
        profile_id: ProfileId,
    },
}

#[derive(Debug, StructOpt)]
pub enum CreateCommand {
    #[structopt(name = "profile")]
    /// Create profile
    Profile, // TODO how to specify to keep current or new profile should be active/default

    #[structopt(name = "link")]
    /// Create link, i.e. follow/subscribe to a remote profile
    Link {
        #[structopt(long = "my_profile_id")]
        /// Add link to this profile of yours if other than the active one
        my_profile_id: Option<ProfileId>,

        #[structopt(long = "peer_profile_id")]
        /// Create link to this remote profile
        peer_profile_id: ProfileId,
        // TODO is an optional "relation_type" needed here?
    },
}

#[derive(Debug, StructOpt)]
pub enum RemoveCommand {
    #[structopt(name = "link")]
    /// Remove link, i.e. unfollow/unsubscribe from another profile
    Link {
        #[structopt(long = "my_profile_id")]
        /// Remove link from this profile of yours if other than the active one
        my_profile_id: Option<ProfileId>,

        #[structopt(long = "peer_profile_id")]
        /// Remove link with this remote profile
        peer_profile_id: ProfileId,
    },
}

#[derive(Debug, StructOpt)]
pub enum SetCommand {
    #[structopt(name = "active-profile")]
    /// Show profile
    ActiveProfile {
        // TODO is activation by profile NUMBER needed or is this enough?
        #[structopt(long = "my_profile_id")]
        /// Profile id to be activated
        my_profile_id: ProfileId,
    },

    #[structopt(name = "attribute")]
    /// Set attribute with name to specified value
    Attribute {
        #[structopt(long = "my_profile_id")]
        /// Set attribute to this profile of yours if other than the active one
        my_profile_id: Option<ProfileId>,

        #[structopt(long = "key")]
        /// Attribute name
        key: AttributeId,

        #[structopt(long = "value")]
        /// Attribute value
        value: AttributeValue,
    },
}

#[derive(Debug, StructOpt)]
pub enum ClearCommand {
    #[structopt(name = "attribute")]
    /// Clear attribute
    Attribute {
        #[structopt(long = "my_profile_id")]
        /// Clear attribute from this profile of yours if other than the active one
        my_profile_id: Option<ProfileId>,

        #[structopt(long = "key")]
        /// Attribute name
        key: AttributeId,
    },
}
