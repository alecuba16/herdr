use std::path::PathBuf;

use crate::api::schema::{
    EventData, EventEnvelope, EventKind, ResponseResult, WorkspaceCreateParams,
    WorkspaceRenameParams, WorkspaceTarget,
};
use crate::app::App;

use super::responses::{encode_error, encode_success};

struct WorkspaceGitSpaceForGrouping {
    ws_idx: usize,
    key: String,
    label: String,
    checkout_path: PathBuf,
    parent_checkout_path: PathBuf,
    is_linked_worktree: bool,
}

impl App {
    pub(super) fn handle_workspace_list(&mut self, id: String) -> String {
        encode_success(
            id,
            ResponseResult::WorkspaceList {
                workspaces: self
                    .state
                    .workspaces
                    .iter()
                    .enumerate()
                    .map(|(idx, _)| self.workspace_info(idx))
                    .collect(),
            },
        )
    }

    pub(super) fn handle_workspace_get(&mut self, id: String, target: WorkspaceTarget) -> String {
        let Some(index) = self.parse_workspace_id(&target.workspace_id) else {
            return workspace_not_found(id, &target.workspace_id);
        };
        let Some(_) = self.state.workspaces.get(index) else {
            return workspace_not_found(id, &target.workspace_id);
        };

        encode_success(
            id,
            ResponseResult::WorkspaceInfo {
                workspace: self.workspace_info(index),
            },
        )
    }

    pub(super) fn handle_workspace_create(
        &mut self,
        id: String,
        params: WorkspaceCreateParams,
    ) -> String {
        let cwd = params.cwd.map(PathBuf::from).unwrap_or_else(|| {
            let follow_cwd = self
                .workspace_creation_source()
                .and_then(|ws_idx| self.seed_cwd_from_workspace(ws_idx));
            self.resolve_new_terminal_cwd(follow_cwd)
        });
        let extra_env = match super::env::normalize_launch_env(params.env) {
            Ok(env) => env,
            Err((code, message)) => return encode_error(id, &code, message),
        };
        match self.create_workspace_with_launch_env(cwd, params.focus, extra_env) {
            Ok(index) => {
                self.infer_workspace_worktree_group(index);
                if let Some(label) = params.label {
                    if let Some(workspace) = self.state.workspaces.get_mut(index) {
                        workspace.set_custom_name(label);
                        crate::logging::workspace_renamed(&workspace.id);
                    }
                }
                self.emit_workspace_open_events(index);
                encode_success(
                    id,
                    self.workspace_created_result(index)
                        .expect("new workspace should produce a complete create response"),
                )
            }
            Err(err) => encode_error(id, "workspace_create_failed", err.to_string()),
        }
    }

    fn infer_workspace_worktree_group(&mut self, created_idx: usize) {
        let Some(created_space) = self.workspace_git_space_for_grouping(created_idx) else {
            return;
        };

        let same_repo_spaces = self
            .state
            .workspaces
            .iter()
            .enumerate()
            .filter_map(|(ws_idx, _)| self.workspace_git_space_for_grouping(ws_idx))
            .filter(|space| space.key == created_space.key)
            .collect::<Vec<_>>();
        if same_repo_spaces.len() < 2
            || !same_repo_spaces
                .iter()
                .any(|space| !space.is_linked_worktree)
        {
            return;
        }

        for space in same_repo_spaces {
            let membership = crate::workspace::WorktreeSpaceMembership {
                key: space.key,
                label: space.label,
                repo_root: space.parent_checkout_path,
                checkout_path: space.checkout_path,
                is_linked_worktree: space.is_linked_worktree,
            };
            self.set_worktree_membership(space.ws_idx, membership, space.ws_idx != created_idx);
        }
    }

    fn workspace_git_space_for_grouping(
        &self,
        ws_idx: usize,
    ) -> Option<WorkspaceGitSpaceForGrouping> {
        let ws = self.state.workspaces.get(ws_idx)?;
        if let Some(membership) = ws.worktree_space() {
            return Some(WorkspaceGitSpaceForGrouping {
                ws_idx,
                key: membership.key.clone(),
                label: membership.label.clone(),
                checkout_path: membership.checkout_path.clone(),
                parent_checkout_path: membership.repo_root.clone(),
                is_linked_worktree: membership.is_linked_worktree,
            });
        }

        let space = ws.git_space().cloned().or_else(|| {
            ws.resolved_identity_cwd_from(&self.state.terminals, &self.terminal_runtimes)
                .as_deref()
                .and_then(crate::workspace::git_space_metadata)
        })?;
        let parent_checkout_path = parent_checkout_path_for_space(&space);
        Some(WorkspaceGitSpaceForGrouping {
            ws_idx,
            key: space.key,
            label: space.label,
            checkout_path: space.repo_root,
            parent_checkout_path,
            is_linked_worktree: space.is_linked_worktree,
        })
    }

    pub(super) fn handle_workspace_focus(&mut self, id: String, target: WorkspaceTarget) -> String {
        let Some(index) = self.parse_workspace_id(&target.workspace_id) else {
            return workspace_not_found(id, &target.workspace_id);
        };
        if self.state.workspaces.get(index).is_none() {
            return workspace_not_found(id, &target.workspace_id);
        }
        self.state.switch_workspace(index);

        encode_success(
            id,
            ResponseResult::WorkspaceInfo {
                workspace: self.workspace_info(index),
            },
        )
    }

    pub(super) fn handle_workspace_rename(
        &mut self,
        id: String,
        params: WorkspaceRenameParams,
    ) -> String {
        let Some(index) = self.parse_workspace_id(&params.workspace_id) else {
            return workspace_not_found(id, &params.workspace_id);
        };
        let Some(ws) = self.state.workspaces.get_mut(index) else {
            return workspace_not_found(id, &params.workspace_id);
        };
        ws.set_custom_name(params.label.clone());
        crate::logging::workspace_renamed(&ws.id);
        self.schedule_session_save();
        self.emit_event(EventEnvelope {
            event: EventKind::WorkspaceRenamed,
            data: EventData::WorkspaceRenamed {
                workspace_id: self.public_workspace_id(index),
                label: params.label,
            },
        });

        encode_success(
            id,
            ResponseResult::WorkspaceInfo {
                workspace: self.workspace_info(index),
            },
        )
    }

    pub(super) fn handle_workspace_close(&mut self, id: String, target: WorkspaceTarget) -> String {
        let Some(index) = self.parse_workspace_id(&target.workspace_id) else {
            return workspace_not_found(id, &target.workspace_id);
        };
        if self.state.workspaces.get(index).is_none() {
            return workspace_not_found(id, &target.workspace_id);
        }
        let workspace_id = self.public_workspace_id(index);
        let workspace = self.workspace_info(index);
        let pane_ids = self
            .state
            .workspaces
            .get(index)
            .map(|ws| {
                ws.tabs
                    .iter()
                    .flat_map(|tab| tab.layout.pane_ids())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        self.state.selected = index;
        self.state.close_selected_workspace();
        for pane_id in pane_ids {
            self.state.plugin_panes.remove(&pane_id);
        }
        self.shutdown_detached_terminal_runtimes();
        self.emit_event(EventEnvelope {
            event: EventKind::WorkspaceClosed,
            data: EventData::WorkspaceClosed {
                workspace_id,
                workspace: Some(workspace),
            },
        });

        encode_success(id, ResponseResult::Ok {})
    }
}

fn parent_checkout_path_for_space(space: &crate::workspace::GitSpaceMetadata) -> PathBuf {
    if !space.is_linked_worktree {
        return space.repo_root.clone();
    }

    crate::worktree::list_existing_worktrees(&space.repo_root)
        .ok()
        .and_then(|entries| {
            entries.into_iter().find_map(|entry| {
                let entry_space = crate::workspace::git_space_metadata(&entry.path)?;
                if entry_space.key == space.key && !entry_space.is_linked_worktree {
                    Some(entry_space.repo_root)
                } else {
                    None
                }
            })
        })
        .unwrap_or_else(|| space.repo_root.clone())
}

#[cfg(test)]
mod workspace_create_tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::api::schema::{Request, SuccessResponse, WorkspaceCreateParams};
    use crate::{config::Config, workspace::Workspace};

    fn unique_temp_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("herdr-{name}-{}-{nanos}", std::process::id()))
    }

    fn run_git(repo: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .status()
            .unwrap();
        assert!(
            status.success(),
            "git command failed: git -C {} {}",
            repo.display(),
            args.join(" ")
        );
    }

    fn create_committed_repo(name: &str) -> PathBuf {
        let repo = unique_temp_path(name);
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "--quiet"]);
        run_git(&repo, &["config", "user.email", "herdr@example.invalid"]);
        run_git(&repo, &["config", "user.name", "Herdr Test"]);
        std::fs::write(repo.join("README.md"), "test\n").unwrap();
        run_git(&repo, &["add", "README.md"]);
        run_git(&repo, &["commit", "--quiet", "-m", "initial"]);
        repo
    }

    fn test_app() -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        App::new(
            &Config::default(),
            true,
            None,
            api_rx,
            crate::api::EventHub::default(),
        )
    }

    #[cfg(windows)]
    fn test_shell() -> &'static str {
        "C:\\Windows\\System32\\whoami.exe"
    }

    #[cfg(not(windows))]
    fn test_shell() -> &'static str {
        "/usr/bin/true"
    }

    #[tokio::test]
    async fn workspace_create_converts_existing_worktree_checkout_into_group() {
        let repo = create_committed_repo("workspace-create-worktree-group-repo");
        let checkout = unique_temp_path("workspace-create-worktree-group-checkout");
        run_git(
            &repo,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "worktree/api-group",
                checkout.to_str().unwrap(),
                "HEAD",
            ],
        );
        let mut app = test_app();
        app.state.default_shell = test_shell().into();
        let mut child = Workspace::test_new("child");
        child.identity_cwd = checkout.clone();
        app.state.workspaces = vec![child];
        app.state.ensure_test_terminals();

        let response = app.handle_api_request(Request {
            id: "req".into(),
            method: crate::api::schema::Method::WorkspaceCreate(WorkspaceCreateParams {
                cwd: Some(repo.display().to_string()),
                focus: false,
                label: None,
                env: HashMap::new(),
            }),
        });

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let crate::api::schema::ResponseResult::WorkspaceCreated { workspace, .. } = success.result
        else {
            panic!("expected workspace_created response");
        };
        assert_eq!(app.state.workspaces.len(), 2);
        assert!(
            app.state.workspaces[0]
                .worktree_space()
                .unwrap()
                .is_linked_worktree
        );
        assert!(
            !app.state.workspaces[1]
                .worktree_space()
                .unwrap()
                .is_linked_worktree
        );
        assert!(workspace
            .worktree
            .as_ref()
            .is_some_and(|worktree| !worktree.is_linked_worktree));

        for (_, runtime) in app.terminal_runtimes.drain() {
            runtime.shutdown();
        }
        let remove = crate::worktree::build_worktree_remove_command(&repo, &checkout, false);
        crate::worktree::run_worktree_command(&remove).unwrap();
        let _ = std::fs::remove_dir_all(repo);
    }
}

fn workspace_not_found(id: String, workspace_id: &str) -> String {
    encode_error(
        id,
        "workspace_not_found",
        format!("workspace {workspace_id} not found"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{api::schema::SuccessResponse, config::Config, workspace::Workspace};

    fn app_with_linked_worktree() -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &Config::default(),
            true,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state.workspaces = vec![Workspace::test_new("issue")];
        app.state.workspaces[0].worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
            key: "repo-key".into(),
            label: "herdr".into(),
            repo_root: "/repo/herdr".into(),
            checkout_path: "/repo/herdr-issue".into(),
            is_linked_worktree: true,
        });
        app
    }

    #[test]
    fn api_workspace_close_closes_linked_worktree_workspace_only() {
        let mut app = app_with_linked_worktree();

        let response = app.handle_workspace_close(
            "req".into(),
            WorkspaceTarget {
                workspace_id: app.state.workspaces[0].id.clone(),
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(success.id, "req");
        assert_eq!(app.state.request_remove_linked_worktree, None);
        assert!(app.state.workspaces.is_empty());
    }

    #[test]
    fn api_workspace_close_event_includes_final_worktree_snapshot() {
        let event_hub = crate::api::EventHub::default();
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(&Config::default(), true, None, api_rx, event_hub.clone());
        app.state.workspaces = app_with_linked_worktree().state.workspaces;
        let workspace_id = app.state.workspaces[0].id.clone();

        let response = app.handle_workspace_close(
            "req".into(),
            WorkspaceTarget {
                workspace_id: workspace_id.clone(),
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(success.id, "req");
        let events = event_hub.events_after(0);
        assert!(events.iter().any(|(_, event)| {
            matches!(
                &event.data,
                EventData::WorkspaceClosed {
                    workspace_id: closed_id,
                    workspace: Some(workspace),
                } if closed_id == &workspace_id
                    && workspace
                        .worktree
                        .as_ref()
                        .is_some_and(|worktree| worktree.is_linked_worktree)
            )
        }));
    }
}
