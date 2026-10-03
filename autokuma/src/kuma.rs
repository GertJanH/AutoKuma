use crate::{app_state::AppState, entity::Entity, error::Result, name::Name, util::fill_templates};
use futures_util::future::join_all;
use itertools::Itertools;
use kuma_client::{
    docker_host::DockerHost,
    monitor::Monitor,
    notification::Notification,
    status_page::{PublicGroupMonitor, StatusPage},
    tag::{Tag, TagDefinition},
    Client,
};
use log::{info, warn};
use std::collections::{HashMap, HashSet};

/// Color for tags created on demand by `kuma_tags`; can be changed in Uptime Kuma afterwards.
const DEFAULT_TAG_COLOR: &str = "#4B5563";

pub fn get_kuma_labels(
    state: &AppState,
    labels: Option<&HashMap<String, String>>,
    template_values: &tera::Context,
) -> Result<Vec<(String, String)>> {
    labels.as_ref().map_or_else(
        || Ok(vec![]),
        |labels| {
            labels
                .iter()
                .filter(|(key, _)| {
                    key.starts_with(&format!("{}.", state.config.docker.label_prefix))
                })
                .map(|(key, value)| {
                    fill_templates(
                        state.config.clone(),
                        key.trim_start_matches(&format!("{}.", state.config.docker.label_prefix)),
                        &template_values,
                    )
                    .map(|key| (key, value.to_owned()))
                })
                .chain(
                    labels
                        .iter()
                        .filter(|(key, _)| state.config.snippets.contains_key(&format!("!{}", key)))
                        .map(|(key, value)| Ok((format!("__!{}", key), value.to_owned()))),
                )
                .collect::<Result<Vec<_>>>()
        },
    )
}

async fn get_managed_docker_hosts(
    state: &AppState,
    kuma: &Client,
) -> Result<HashMap<String, DockerHost>> {
    let map = state
        .db
        .get_docker_hosts()?
        .into_iter()
        .map(|(key, value)| (value, key))
        .collect::<HashMap<_, _>>();

    Ok(kuma
        .get_docker_hosts()
        .await?
        .into_iter()
        .filter_map(|docker_host| {
            map.get(&docker_host.id.unwrap_or(-1))
                .map(|id| (id.to_owned(), docker_host))
        })
        .collect::<HashMap<_, _>>())
}

async fn get_managed_notification_providers(
    state: &AppState,
    kuma: &Client,
) -> Result<HashMap<String, Notification>> {
    let map = state
        .db
        .get_notifications()?
        .into_iter()
        .map(|(key, value)| (value, key))
        .collect::<HashMap<_, _>>();

    Ok(kuma
        .get_notifications()
        .await?
        .into_iter()
        .filter_map(|docker_host| {
            map.get(&docker_host.id.unwrap_or(-1))
                .map(|id| (id.to_owned(), docker_host))
        })
        .collect::<HashMap<_, _>>())
}

async fn get_managed_tags(
    state: &AppState,
    kuma: &Client,
) -> Result<HashMap<String, TagDefinition>> {
    let map = state
        .db
        .get_tags()?
        .into_iter()
        .map(|(key, value)| (value, key))
        .collect::<HashMap<_, _>>();

    Ok(kuma
        .get_tags()
        .await?
        .into_iter()
        .filter_map(|tag| {
            map.get(&tag.tag_id.unwrap_or(-1))
                .map(|id| (id.to_owned(), tag))
        })
        .collect::<HashMap<_, _>>())
}

async fn get_managed_status_pages(
    state: &AppState,
    kuma: &Client,
) -> Result<HashMap<String, StatusPage>> {
    let map = state
        .db
        .get_status_pages()?
        .into_iter()
        .map(|(key, value)| (value, key))
        .collect::<HashMap<_, _>>();

    Ok(join_all(
        kuma.get_status_pages()
            .await?
            .into_iter()
            .filter_map(|(_, status_page)| status_page.slug)
            .map(|slug| kuma.get_status_page(slug)),
    )
    .await
    .into_iter()
    .flatten()
    .filter_map(|status_page| {
        map.get(&status_page.slug.clone().unwrap_or_default())
            .map(|id| (id.to_owned(), status_page))
    })
    .collect::<HashMap<_, _>>())
}

async fn get_managed_monitors(state: &AppState, kuma: &Client) -> Result<HashMap<String, Monitor>> {
    let map = state
        .db
        .get_monitors()?
        .into_iter()
        .map(|(key, value)| (value, key))
        .collect::<HashMap<_, _>>();

    Ok(kuma
        .get_monitors()
        .await?
        .into_iter()
        .filter_map(|(_, monitor)| {
            map.get(&monitor.common().id().unwrap_or(-1))
                .map(|id| (id.to_owned(), monitor))
        })
        .collect::<HashMap<_, _>>())
}

pub async fn get_managed_entities(
    state: &AppState,
    kuma: &Client,
) -> Result<HashMap<String, Entity>> {
    Ok(get_managed_monitors(&state, &kuma)
        .await?
        .into_iter()
        .map(|(id, monitor)| (id, Entity::Monitor(monitor)))
        .chain(
            get_managed_docker_hosts(&state, &kuma)
                .await?
                .into_iter()
                .map(|(id, host)| (id, Entity::DockerHost(host))),
        )
        .chain(
            get_managed_notification_providers(&state, &kuma)
                .await?
                .into_iter()
                .map(|(id, notification)| (id, Entity::Notification(notification))),
        )
        .chain(
            get_managed_tags(&state, &kuma)
                .await?
                .into_iter()
                .map(|(id, tag)| (id, Entity::Tag(tag))),
        )
        .chain(
            get_managed_status_pages(&state, &kuma)
                .await?
                .into_iter()
                .map(|(id, status_page)| (id, Entity::StatusPage(status_page))),
        )
        .collect::<HashMap<_, _>>())
}

/// Resolves each monitor's `kuma_tags` (Uptime Kuma tag *names*) into tag ids,
/// creating tags that don't exist yet. Runs before the diff, so the resolved
/// tags count as desired state.
pub async fn resolve_kuma_tags(kuma: &Client, entities: &mut HashMap<String, Entity>) -> Result<()> {
    let mut known_tags: Option<Vec<TagDefinition>> = None;

    for (id, entity) in entities.iter_mut() {
        let Entity::Monitor(monitor) = entity else {
            continue;
        };
        let Some(wanted) = monitor.common().kuma_tags().clone() else {
            continue;
        };

        let known = match &mut known_tags {
            Some(known) => known,
            None => known_tags.insert(kuma.get_tags().await?),
        };

        for tag in wanted {
            let matches = known
                .iter()
                .filter(|t| t.name.as_deref() == Some(tag.name.as_str()))
                .filter_map(|t| t.tag_id)
                .sorted()
                .collect_vec();

            if matches.len() > 1 {
                warn!(
                    "{}: multiple Uptime Kuma tags named '{}', using the oldest (id {})",
                    id, tag.name, matches[0]
                );
            }

            let tag_id = match matches.first() {
                Some(tag_id) => *tag_id,
                None => {
                    let created = kuma
                        .add_tag(TagDefinition {
                            tag_id: None,
                            name: Some(tag.name.clone()),
                            color: Some(DEFAULT_TAG_COLOR.to_owned()),
                        })
                        .await;
                    match created {
                        Ok(created) if created.tag_id.is_some() => {
                            info!("Created tag: {}", tag.name);
                            let tag_id = created.tag_id.unwrap();
                            known.push(created);
                            tag_id
                        }
                        other => {
                            warn!("{}: failed to create tag '{}': {:?}", id, tag.name, other.err());
                            continue;
                        }
                    }
                }
            };

            let tags = monitor.common_mut().tags_mut();
            if !tags.iter().any(|t| t.tag_id == Some(tag_id)) {
                tags.push(Tag {
                    tag_id: Some(tag_id),
                    value: tag.value.clone(),
                    ..Default::default()
                });
            }
        }
    }

    Ok(())
}

/// Adds monitors with `kuma_status_pages` ("slug:Group") to that group of an
/// existing status page. Add-only: never removes or moves a monitor that is
/// already anywhere on the page, and only saves a page when something is
/// missing, so a lost update (e.g. two AutoKuma instances saving the same page)
/// is simply redone on the next sync.
// ponytail: reads every referenced page on every sync; cache per page if Kuma load ever matters.
pub async fn sync_status_pages(
    state: &AppState,
    kuma: &Client,
    entities: &HashMap<String, Entity>,
) -> Result<()> {
    let mut wanted: HashMap<String, HashMap<String, Vec<i32>>> = HashMap::new();

    for (id, entity) in entities {
        let Entity::Monitor(monitor) = entity else {
            continue;
        };
        let Some(refs) = monitor.common().kuma_status_pages() else {
            continue;
        };
        // Not created in Kuma yet: picked up on a later sync.
        let Some(monitor_id) = state
            .db
            .get_id::<i32>(Name::Monitor(id.clone()))
            .ok()
            .flatten()
        else {
            continue;
        };

        for page in refs {
            match &page.value {
                Some(group) => wanted
                    .entry(page.name.clone())
                    .or_default()
                    .entry(group.clone())
                    .or_default()
                    .push(monitor_id),
                None => warn!(
                    "{}: kuma_status_pages entry '{}' needs the form 'slug:Group'",
                    id, page.name
                ),
            }
        }
    }

    for (slug, groups) in wanted {
        let mut page: StatusPage = match kuma.get_status_page(&slug).await {
            Ok(page) => page,
            Err(e) => {
                warn!("Unable to read status page '{}': {}", slug, e);
                continue;
            }
        };

        let on_page = page
            .public_group_list
            .iter()
            .flatten()
            .flat_map(|group| group.monitor_list.iter())
            .filter_map(|monitor| monitor.id)
            .collect::<HashSet<_>>();

        let mut changed = false;
        for (group_name, monitor_ids) in groups {
            let Some(group) = page
                .public_group_list
                .iter_mut()
                .flatten()
                .find(|group| group.name.as_deref() == Some(group_name.as_str()))
            else {
                warn!(
                    "Status page '{}' has no group '{}' (create it once in Uptime Kuma)",
                    slug, group_name
                );
                continue;
            };

            for monitor_id in monitor_ids.into_iter().filter(|id| !on_page.contains(id)) {
                info!(
                    "Adding monitor {} to status page {} / {}",
                    monitor_id, slug, group_name
                );
                group.monitor_list.push(PublicGroupMonitor {
                    id: Some(monitor_id),
                    ..Default::default()
                });
                changed = true;
            }
        }

        if changed {
            if let Err(e) = kuma.edit_status_page(page).await {
                warn!("Unable to save status page '{}': {}", slug, e);
            }
        }
    }

    Ok(())
}
