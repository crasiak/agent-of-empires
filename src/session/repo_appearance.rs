//! Shared repository labels, keyed by repository path or a synthetic bucket ID.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const SCRATCH_REPO_ID: &str = "__scratch__";
pub const MULTI_REPO_ID: &str = "__multi_repo__";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepoColor {
    Amber,
    Teal,
    Sky,
    Violet,
    Rose,
    Slate,
}

impl RepoColor {
    pub const ALL: [Self; 6] = [
        Self::Amber,
        Self::Teal,
        Self::Sky,
        Self::Violet,
        Self::Rose,
        Self::Slate,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Amber => "Amber",
            Self::Teal => "Teal",
            Self::Sky => "Sky",
            Self::Violet => "Violet",
            Self::Rose => "Rose",
            Self::Slate => "Slate",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoAppearance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<RepoColor>,
}
pub type RepoAppearances = BTreeMap<String, RepoAppearance>;

fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoAppearancePatch {
    pub repo_path: String,
    #[serde(default, deserialize_with = "present")]
    pub alias: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    pub color: Option<Option<RepoColor>>,
}
impl RepoAppearancePatch {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            valid_repo_id(&self.repo_path),
            "repo_path must be an absolute path or a synthetic repository ID"
        );
        Ok(())
    }
    pub fn apply(self, map: &mut RepoAppearances) {
        let entry = map.entry(self.repo_path.clone()).or_default();
        if let Some(alias) = self.alias {
            entry.alias = alias
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
        }
        if let Some(color) = self.color {
            entry.color = color;
        }
        // Empty entries are tombstones: a stale browser migration must not resurrect clears.
    }
}
pub fn valid_repo_id(id: &str) -> bool {
    !id.contains('\0')
        && (id == SCRATCH_REPO_ID || id == MULTI_REPO_ID || std::path::Path::new(id).is_absolute())
}

pub fn instance_repo_id(instance: &super::Instance) -> &str {
    if instance.scratch {
        SCRATCH_REPO_ID
    } else if instance
        .workspace_info
        .as_ref()
        .is_some_and(|w| w.repos.len() > 1)
    {
        MULTI_REPO_ID
    } else {
        instance.repo_path()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn patches_preserve_absent_fields_and_other_repositories() {
        let mut map = RepoAppearances::new();
        for value in [
            serde_json::json!({"repo_path":"/a", "alias":" A ", "color":"sky"}),
            serde_json::json!({"repo_path":"/b", "color":"rose"}),
            serde_json::json!({"repo_path":"/a", "color":null}),
        ] {
            let patch: RepoAppearancePatch = serde_json::from_value(value).unwrap();
            patch.validate().unwrap();
            patch.apply(&mut map);
        }
        assert_eq!(map["/a"].alias.as_deref(), Some("A"));
        assert_eq!(map["/a"].color, None);
        assert_eq!(map["/b"].color, Some(RepoColor::Rose));
        for value in [
            serde_json::json!({"repo_path":"/a", "color":"green"}),
            serde_json::json!({"repo_path":"/a", "oops":true}),
        ] {
            assert!(serde_json::from_value::<RepoAppearancePatch>(value).is_err());
        }
        assert!(!valid_repo_id("basename"));
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoAppearanceImport {
    pub appearances: RepoAppearances,
}
impl RepoAppearanceImport {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.appearances.keys().all(|id| valid_repo_id(id)),
            "repository IDs must be absolute paths or synthetic bucket IDs"
        );
        Ok(())
    }
    pub fn apply(self, map: &mut RepoAppearances) {
        for (id, mut appearance) in self.appearances {
            appearance.alias = appearance
                .alias
                .map(|a| a.trim().to_string())
                .filter(|a| !a.is_empty());
            map.entry(id).or_insert(appearance);
        }
    }
}
