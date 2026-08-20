//! Service-tier selection and model-catalog helpers for `ChatWidget`.

use super::ChatWidget;
use crate::app_command::AppCommand;
use crate::app_event::AppEvent;
use crate::bottom_pane::SelectionAction;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionViewParams;
use crate::bottom_pane::popup_consts::standard_popup_hint_line;
use crate::bottom_pane::slash_commands::ServiceTierCommand;
use crate::service_tier_resolution;
use codex_features::Feature;
use codex_protocol::config_types::SERVICE_TIER_DEFAULT_REQUEST_VALUE;
use codex_protocol::config_types::ServiceTier;
use codex_protocol::openai_models::SPEED_TIER_FAST;

impl ChatWidget {
    pub(crate) fn set_service_tier(&mut self, service_tier: Option<String>) {
        self.config.service_tier = service_tier;
        self.refresh_effective_service_tier();
        self.refresh_model_dependent_surfaces();
    }

    pub(crate) fn current_service_tier(&self) -> Option<&str> {
        self.effective_service_tier.as_deref()
    }

    pub(crate) fn configured_service_tier(&self) -> Option<String> {
        self.config.service_tier.clone()
    }

    pub(crate) fn service_tier_update_for_core(&self) -> Option<Option<String>> {
        service_tier_resolution::service_tier_update_for_core(
            &self.config,
            self.current_model(),
            &self.model_catalog.try_list_models().unwrap_or_default(),
        )
    }

    pub(crate) fn should_show_fast_status(&self, model: &str, service_tier: Option<&str>) -> bool {
        service_tier.is_some_and(|service_tier| {
            service_tier == ServiceTier::Fast.request_value()
                && self.model_supports_service_tier(model, service_tier)
        }) && self.has_chatgpt_account
    }

    pub(super) fn fast_mode_enabled(&self) -> bool {
        self.config.features.enabled(Feature::FastMode)
    }

    pub(crate) fn can_toggle_fast_mode_from_keybinding(&self) -> bool {
        self.fast_mode_enabled()
            && self.current_model_fast_service_tier().is_some()
            && !self.is_user_turn_pending_or_running()
            && self.bottom_pane.no_modal_or_popup_active()
    }

    pub(crate) fn toggle_fast_mode_from_ui(&mut self) {
        let Some(fast_tier) = self.current_model_fast_service_tier() else {
            return;
        };
        let next_tier = if self.current_service_tier() == Some(fast_tier.id.as_str()) {
            Some(SERVICE_TIER_DEFAULT_REQUEST_VALUE.to_string())
        } else {
            Some(fast_tier.id)
        };
        self.set_service_tier_selection(next_tier);
    }

    pub(crate) fn toggle_service_tier_from_ui(&mut self, command: ServiceTierCommand) {
        let next_tier = if self.current_service_tier() == Some(command.id.as_str()) {
            Some(SERVICE_TIER_DEFAULT_REQUEST_VALUE.to_string())
        } else {
            Some(command.id)
        };
        self.set_service_tier_selection(next_tier);
    }

    pub(super) fn sync_service_tier_commands(&mut self) {
        self.bottom_pane
            .set_service_tier_commands_enabled(self.fast_mode_enabled());
        self.bottom_pane
            .set_service_tier_commands(self.current_model_service_tier_commands());
    }

    pub(super) fn current_model_service_tier_commands(&self) -> Vec<ServiceTierCommand> {
        let model = self.current_model();
        self.model_catalog
            .try_list_models()
            .ok()
            .and_then(|models| {
                models
                    .into_iter()
                    .find(|preset| preset.model == model)
                    .map(|preset| {
                        preset
                            .service_tiers
                            .into_iter()
                            .map(|tier| ServiceTierCommand {
                                id: tier.id,
                                name: tier.name.to_lowercase(),
                                description: tier.description,
                            })
                            .collect()
                    })
            })
            .unwrap_or_default()
    }

    fn set_service_tier_selection(&mut self, service_tier: Option<String>) {
        // Apply immediately for the *current session only*. This does not
        // touch config.toml — see `open_service_tier_scope_prompt` below for
        // the (opt-in) path that persists the choice as a future default.
        self.set_service_tier(service_tier.clone());
        self.app_event_tx
            .send(AppEvent::CodexOp(AppCommand::override_turn_context(
                /*cwd*/ None,
                /*approval_policy*/ None,
                /*approvals_reviewer*/ None,
                /*permission_profile*/ None,
                /*active_permission_profile*/ None,
                /*windows_sandbox_level*/ None,
                /*model*/ None,
                /*effort*/ None,
                /*summary*/ None,
                Some(service_tier.clone()),
                /*collaboration_mode*/ None,
                /*personality*/ None,
            )));
        self.app_event_tx
            .send(AppEvent::OpenServiceTierScopePrompt { service_tier });
    }

    /// Ask the user whether the service-tier change they just made for this
    /// session should also become the default for future sessions. Persisting
    /// to config.toml only happens if they explicitly choose "Save as default".
    pub(crate) fn open_service_tier_scope_prompt(&mut self, service_tier: Option<String>) {
        let label = service_tier
            .as_deref()
            .unwrap_or(SERVICE_TIER_DEFAULT_REQUEST_VALUE)
            .to_string();
        let subtitle = format!("Using \"{label}\" for this session. Save it as your default?");

        let session_only_actions: Vec<SelectionAction> = vec![Box::new(|_tx| {
            // No-op: already applied to the current session above.
        })];

        let save_default_actions: Vec<SelectionAction> = vec![Box::new({
            let service_tier = service_tier.clone();
            move |tx| {
                tx.send(AppEvent::PersistServiceTierSelection {
                    service_tier: service_tier.clone(),
                });
            }
        })];

        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: Some("Service tier".to_string()),
            subtitle: Some(subtitle),
            footer_hint: Some(standard_popup_hint_line()),
            items: vec![
                SelectionItem {
                    name: "Use for this session only".to_string(),
                    description: Some("Default value stays as-is for new sessions.".to_string()),
                    actions: session_only_actions,
                    dismiss_on_select: true,
                    ..Default::default()
                },
                SelectionItem {
                    name: "Save as default".to_string(),
                    description: Some(
                        "Writes service_tier to config.toml for future sessions.".to_string(),
                    ),
                    actions: save_default_actions,
                    dismiss_on_select: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
    }

    fn model_supports_service_tier(&self, model: &str, service_tier: &str) -> bool {
        self.model_catalog
            .try_list_models()
            .ok()
            .and_then(|models| {
                models
                    .into_iter()
                    .find(|preset| preset.model == model)
                    .map(|preset| {
                        preset
                            .service_tiers
                            .iter()
                            .any(|tier| tier.id == service_tier)
                    })
            })
            .unwrap_or(false)
    }

    fn current_model_fast_service_tier(&self) -> Option<ServiceTierCommand> {
        self.current_model_service_tier_commands()
            .into_iter()
            .find(|tier| tier.name.eq_ignore_ascii_case(SPEED_TIER_FAST))
    }

    pub(super) fn refresh_effective_service_tier(&mut self) {
        self.effective_service_tier = service_tier_resolution::effective_service_tier(
            &self.config,
            self.current_model(),
            &self.model_catalog.try_list_models().unwrap_or_default(),
        );
    }
}
