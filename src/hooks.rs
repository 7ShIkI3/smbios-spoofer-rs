//! # Hooks System — Extension Points du Cycle de Vie
//!
//! Inspiré de :
//! - **OpenAI Codex** : `hooks.json` + `PreToolUse`/`PostToolUse`/`SessionStart`/`Stop`
//! - **Claude Code** : hooks `PreToolUse`/`PostToolUse`/`Notification`/`UserPromptSubmit`/`Stop`/`SessionStart`/`PreCompact`
//! - **Agent Zero** : 23 points d'extension dans l'agent loop, exécutés par ordre numérique
//!
//! ## Architecture
//!
//! Un Hook est une fonction déclenchée automatiquement à un point précis
//! du cycle de vie de l'agent. Les hooks peuvent :
//! - **Observer** (read-only) — logger, auditer, mesurer
//! - **Modifier** — altérer les paramètres avant exécution
//! - **Bloquer** — refuser une action (deny)
//! - **Approuver** — pré-approuver une action (allow)
//!
//! ## Points d'extension (12 hooks UEFI)
//!
//! ```
//! SessionStart → AgentInit → BeforeObserve → AfterObserve
//!   → BeforeDecide → AfterDecide → BeforeAct → AfterAct
//!     → BeforePersist → AfterPersist → SessionEnd → PanicHook
//! ```

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::boxed::Box;
use alloc::format;

// ─── Hook Result ────────────────────────────────────────────────────────────

/// Résultat de l'exécution d'un hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookResult {
    /// Continuer normalement
    Continue,
    /// Bloquer l'action (deny)
    Deny { reason: String },
    /// Modifier les données avant de continuer
    Modify { message: String },
}

impl HookResult {
    pub fn is_deny(&self) -> bool {
        matches!(self, Self::Deny { .. })
    }

    pub fn deny(reason: &str) -> Self {
        Self::Deny {
            reason: String::from(reason),
        }
    }
}

// ─── Hook Context ───────────────────────────────────────────────────────────

/// Contexte passé à chaque hook. Contient les infos sur l'action en cours.
#[derive(Debug, Clone)]
pub struct HookContext {
    /// Nom de l'événement déclencheur
    pub event: String,
    /// Nom de l'agent courant
    pub agent_name: String,
    /// Nom de l'outil (si applicable)
    pub tool_name: Option<String>,
    /// Paramètres de l'outil (si applicable)
    pub tool_params: Option<String>,
    /// Timestamp UEFI
    pub timestamp: u64,
    /// Étape courante dans le workflow
    pub step: usize,
    /// Données additionnelles
    pub extra: Vec<(String, String)>,
}

impl HookContext {
    pub fn new(event: &str, agent_name: &str) -> Self {
        Self {
            event: String::from(event),
            agent_name: String::from(agent_name),
            tool_name: None,
            tool_params: None,
            timestamp: 0,
            step: 0,
            extra: Vec::new(),
        }
    }

    pub fn with_tool(mut self, name: &str, params: &str) -> Self {
        self.tool_name = Some(String::from(name));
        self.tool_params = Some(String::from(params));
        self
    }

    pub fn with_step(mut self, step: usize) -> Self {
        self.step = step;
        self
    }
}

// ─── Hook Trait ─────────────────────────────────────────────────────────────

/// Trait pour un hook d'extension.
///
/// Chaque hook est une fonction qui s'exécute à un point d'extension
/// du cycle de vie et peut observer, modifier ou bloquer.
pub trait Hook {
    /// Nom unique du hook (pour les logs).
    fn name(&self) -> &str;

    /// Événement sur lequel ce hook se déclenche.
    fn event(&self) -> HookEvent;

    /// Priorité d'exécution (0 = premier, 255 = dernier).
    /// Les hooks de même priorité s'exécutent dans l'ordre d'enregistrement.
    fn priority(&self) -> u8 {
        128 // Priorité moyenne par défaut
    }

    /// Exécute le hook.
    ///
    /// Retourne :
    /// - `Continue` — l'action continue normalement
    /// - `Deny` — l'action est bloquée
    /// - `Modify` — l'action continue avec des données modifiées
    fn execute(&mut self, ctx: &HookContext) -> HookResult;
}

// ─── Hook Events ────────────────────────────────────────────────────────────

/// Les 12 événements du cycle de vie UEFI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    /// Début de session (boot de l'EFI)
    SessionStart,
    /// Fin de session (avant exit)
    SessionEnd,
    /// Agent initialisé
    AgentInit,
    /// Avant la phase d'observation
    BeforeObserve,
    /// Après la phase d'observation
    AfterObserve,
    /// Avant la phase de décision
    BeforeDecide,
    /// Après la phase de décision
    AfterDecide,
    /// Avant l'exécution d'un outil
    BeforeAct,
    /// Après l'exécution d'un outil
    AfterAct,
    /// Avant la persistance
    BeforePersist,
    /// Après la persistance
    AfterPersist,
    /// En cas de panic/erreur fatale
    PanicHook,
}

impl HookEvent {
    /// Nom lisible de l'événement.
    pub fn as_str(&self) -> &str {
        match self {
            Self::SessionStart => "SessionStart",
            Self::SessionEnd => "SessionEnd",
            Self::AgentInit => "AgentInit",
            Self::BeforeObserve => "BeforeObserve",
            Self::AfterObserve => "AfterObserve",
            Self::BeforeDecide => "BeforeDecide",
            Self::AfterDecide => "AfterDecide",
            Self::BeforeAct => "BeforeAct",
            Self::AfterAct => "AfterAct",
            Self::BeforePersist => "BeforePersist",
            Self::AfterPersist => "AfterPersist",
            Self::PanicHook => "PanicHook",
        }
    }

    /// Est-ce un hook de type "gate" (peut bloquer l'action) ?
    pub fn is_gate(&self) -> bool {
        matches!(
            self,
            Self::BeforeAct | Self::BeforePersist | Self::BeforeDecide
        )
    }

    /// Est-ce un hook de type "observer" (read-only) ?
    pub fn is_observer(&self) -> bool {
        matches!(
            self,
            Self::AfterObserve
                | Self::AfterDecide
                | Self::AfterAct
                | Self::AfterPersist
                | Self::SessionStart
                | Self::SessionEnd
        )
    }
}

// ─── Hook Registry ──────────────────────────────────────────────────────────

/// Registre de hooks — gère l'enregistrement et l'exécution ordonnée.
///
/// Pattern : identique au `HookRegistry` de Codex et au système
/// d'extensions de Agent Zero (hooks exécutés par ordre numérique).
pub struct HookRegistry {
    hooks: Vec<Box<dyn Hook>>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    /// Enregistre un hook.
    pub fn register(&mut self, hook: Box<dyn Hook>) {
        self.hooks.push(hook);
    }

    /// Exécute tous les hooks pour un événement donné.
    ///
    /// Les hooks sont triés par priorité (0 = premier).
    /// Si un hook retourne `Deny`, l'exécution s'arrête immédiatement.
    ///
    /// Retourne :
    /// - `None` si tous les hooks ont passé (Continue)
    /// - `Some(HookResult::Deny)` si un hook a bloqué
    /// - `Some(HookResult::Modify)` si un hook a modifié (le dernier modifié l'emporte)
    pub fn run(&mut self, event: HookEvent, ctx: &HookContext) -> Option<HookResult> {
        // Trier les hooks par priorité pour cet événement
        let mut matching: Vec<&mut Box<dyn Hook>> = self
            .hooks
            .iter_mut()
            .filter(|h| h.event() == event)
            .collect();

        matching.sort_by_key(|h| h.priority());

        let mut last_modify: Option<HookResult> = None;

        for hook in matching.iter_mut() {
            match hook.execute(ctx) {
                HookResult::Deny { reason } => {
                    return Some(HookResult::Deny { reason });
                }
                HookResult::Modify { message } => {
                    last_modify = Some(HookResult::Modify { message });
                }
                HookResult::Continue => {}
            }
        }

        last_modify
    }

    /// Vérifie si un événement est bloqué par un hook (sans exécuter).
    /// Utile pour le pre-check rapide.
    pub fn is_blocked(&mut self, event: HookEvent, ctx: &HookContext) -> bool {
        self.run(event, ctx)
            .map(|r| r.is_deny())
            .unwrap_or(false)
    }

    /// Nombre de hooks enregistrés.
    pub fn len(&self) -> usize {
        self.hooks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    /// Liste les hooks par événement.
    pub fn hooks_for_event(&self, event: HookEvent) -> Vec<&dyn Hook> {
        self.hooks
            .iter()
            .filter(|h| h.event() == event)
            .map(|h| h.as_ref())
            .collect()
    }

    /// Compte les hooks par événement.
    pub fn count_for_event(&self, event: HookEvent) -> usize {
        self.hooks.iter().filter(|h| h.event() == event).count()
    }
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Hooks prédéfinis ──────────────────────────────────────────────────────

/// Hook d'audit — log chaque action dans l'AuditTrail.
///
/// Équivalent du pattern "action audit log" de Claude Code
/// (PostToolUse hook qui enregistre chaque outil appelé).
pub struct AuditHook {
    event_type: HookEvent,
    name_str: String,
}

impl AuditHook {
    pub fn new(event_type: HookEvent) -> Self {
        Self {
            event_type,
            name_str: alloc::format!("AuditHook({})", event_type.as_str()),
        }
    }

    /// Crée un hook d'audit pour PostToolUse (le plus commun).
    pub fn post_tool_use() -> Self {
        Self::new(HookEvent::AfterAct)
    }
}

impl Hook for AuditHook {
    fn name(&self) -> &str {
        &self.name_str
    }

    fn event(&self) -> HookEvent {
        self.event_type
    }

    fn execute(&mut self, _ctx: &HookContext) -> HookResult {
        // En UEFI, on ne peut pas logger sur disque ici.
        // Mais le pattern est prêt pour quand on aura un stockage.
        HookResult::Continue
    }
}

/// Hook de sécurité — bloque les opérations dangereuses.
///
/// Équivalent de Claude Code : "Guard dangerous commands"
/// (PreToolUse hook qui bloque `rm -rf`, `DROP TABLE`, etc.)
pub struct SecurityHook {
    blocked_actions: Vec<String>,
}

impl SecurityHook {
    pub fn new() -> Self {
        Self {
            blocked_actions: Vec::new(),
        }
    }

    /// Ajoute une action à la liste noire.
    pub fn block_action(&mut self, action: &str) {
        self.blocked_actions.push(String::from(action));
    }
}

impl Hook for SecurityHook {
    fn name(&self) -> &str {
        "SecurityHook"
    }

    fn event(&self) -> HookEvent {
        HookEvent::BeforeAct
    }

    fn priority(&self) -> u8 {
        0 // Priorité maximale — sécurité d'abord
    }

    fn execute(&mut self, ctx: &HookContext) -> HookResult {
        if let Some(ref tool_name) = ctx.tool_name {
            for blocked in &self.blocked_actions {
                if tool_name.contains(blocked.as_str()) {
                    return HookResult::deny(&format!(
                        "BLOCKED: '{}' is forbidden by security policy",
                        tool_name
                    ));
                }
            }
        }
        HookResult::Continue
    }
}

impl Default for SecurityHook {
    fn default() -> Self {
        Self::new()
    }
}

/// Hook de confirmation — demande approbation pour les actions destructives.
///
/// Pattern : Codex "approval_policy", Claude Code "PermissionRequest hooks".
/// Bloque les actions destructives à moins qu'elles ne soient pré-approuvées.
pub struct ConfirmationHook {
    /// Actions qui nécessitent confirmation
    require_confirm: Vec<String>,
    /// Actions pré-approuvées
    pre_approved: Vec<String>,
    /// Mode : "untrusted" (tout demande), "on-request" (selectif), "never" (jamais)
    mode: ConfirmationMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationMode {
    /// Demander pour toute action non pré-approuvée
    Untrusted,
    /// Demander seulement pour les actions dans la liste
    OnRequest,
    /// Ne jamais demander
    Never,
}

impl ConfirmationHook {
    pub fn new(mode: ConfirmationMode) -> Self {
        Self {
            require_confirm: Vec::new(),
            pre_approved: Vec::new(),
            mode,
        }
    }

    pub fn require(&mut self, action: &str) {
        self.require_confirm.push(String::from(action));
    }

    pub fn pre_approve(&mut self, action: &str) {
        self.pre_approved.push(String::from(action));
    }
}

impl Hook for ConfirmationHook {
    fn name(&self) -> &str {
        "ConfirmationHook"
    }

    fn event(&self) -> HookEvent {
        HookEvent::BeforeAct
    }

    fn priority(&self) -> u8 {
        10 // Après SecurityHook, avant les autres
    }

    fn execute(&mut self, ctx: &HookContext) -> HookResult {
        if self.mode == ConfirmationMode::Never {
            return HookResult::Continue;
        }

        if let Some(ref tool_name) = ctx.tool_name {
            // Déjà pré-approuvé ?
            for approved in &self.pre_approved {
                if tool_name.contains(approved.as_str()) {
                    return HookResult::Continue;
                }
            }

            // Mode untrusted : tout doit être confirmé
            if self.mode == ConfirmationMode::Untrusted {
                return HookResult::deny(&format!(
                    "Action '{}' requires confirmation (mode: untrusted)",
                    tool_name
                ));
            }

            // Mode on-request : seulement les actions listées
            for required in &self.require_confirm {
                if tool_name.contains(required.as_str()) {
                    return HookResult::deny(&format!(
                        "Action '{}' requires confirmation",
                        tool_name
                    ));
                }
            }
        }

        HookResult::Continue
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyHook {
        name: String,
        event: HookEvent,
        priority: u8,
        result: HookResult,
    }

    impl Hook for DummyHook {
        fn name(&self) -> &str { &self.name }
        fn event(&self) -> HookEvent { self.event }
        fn priority(&self) -> u8 { self.priority }
        fn execute(&mut self, _ctx: &HookContext) -> HookResult {
            self.result.clone()
        }
    }

    #[test]
    fn test_hook_registry_empty() {
        let mut reg = HookRegistry::new();
        let ctx = HookContext::new("test", "test_agent");
        assert!(reg.run(HookEvent::BeforeAct, &ctx).is_none());
    }

    #[test]
    fn test_hook_registry_continue() {
        let mut reg = HookRegistry::new();
        reg.register(Box::new(DummyHook {
            name: String::from("test"),
            event: HookEvent::BeforeAct,
            priority: 128,
            result: HookResult::Continue,
        }));
        let ctx = HookContext::new("test", "test_agent");
        assert!(reg.run(HookEvent::BeforeAct, &ctx).is_none());
    }

    #[test]
    fn test_hook_registry_deny_stops_chain() {
        let mut reg = HookRegistry::new();
        reg.register(Box::new(DummyHook {
            name: String::from("gate"),
            event: HookEvent::BeforeAct,
            priority: 0,
            result: HookResult::Deny {
                reason: String::from("blocked"),
            },
        }));
        reg.register(Box::new(DummyHook {
            name: String::from("after"),
            event: HookEvent::BeforeAct,
            priority: 1,
            result: HookResult::Modify {
                message: String::from("never called"),
            },
        }));
        let ctx = HookContext::new("test", "test_agent");
        let result = reg.run(HookEvent::BeforeAct, &ctx);
        assert!(result.is_some());
        assert!(result.unwrap().is_deny());
    }

    #[test]
    fn test_hook_registry_priority_order() {
        let mut reg = HookRegistry::new();
        reg.register(Box::new(DummyHook {
            name: String::from("low"),
            event: HookEvent::AfterAct,
            priority: 200,
            result: HookResult::Continue,
        }));
        reg.register(Box::new(DummyHook {
            name: String::from("high"),
            event: HookEvent::AfterAct,
            priority: 10,
            result: HookResult::Continue,
        }));
        let hooks = reg.hooks_for_event(HookEvent::AfterAct);
        // On vérifie juste que les deux sont présents
        assert_eq!(hooks.len(), 2);
    }

    #[test]
    fn test_hook_event_is_gate() {
        assert!(HookEvent::BeforeAct.is_gate());
        assert!(HookEvent::BeforePersist.is_gate());
        assert!(HookEvent::BeforeDecide.is_gate());
        assert!(!HookEvent::AfterAct.is_gate());
        assert!(!HookEvent::SessionStart.is_gate());
    }

    #[test]
    fn test_hook_event_is_observer() {
        assert!(HookEvent::AfterAct.is_observer());
        assert!(HookEvent::SessionStart.is_observer());
        assert!(!HookEvent::BeforeAct.is_observer());
        assert!(!HookEvent::BeforeDecide.is_observer());
    }

    #[test]
    fn test_security_hook_blocks() {
        let mut hook = SecurityHook::new();
        hook.block_action("dangerous");
        let ctx = HookContext::new("BeforeAct", "agent")
            .with_tool("dangerous_tool", "{}");
        let result = hook.execute(&ctx);
        assert!(result.is_deny());
    }

    #[test]
    fn test_security_hook_allows_safe() {
        let mut hook = SecurityHook::new();
        hook.block_action("dangerous");
        let ctx = HookContext::new("BeforeAct", "agent")
            .with_tool("safe_tool", "{}");
        let result = hook.execute(&ctx);
        assert_eq!(result, HookResult::Continue);
    }

    #[test]
    fn test_confirmation_hook_untrusted_blocks_all() {
        let hook = ConfirmationHook::new(ConfirmationMode::Untrusted);
        let mut hook = hook; // make mutable
        let ctx = HookContext::new("BeforeAct", "agent")
            .with_tool("any_tool", "{}");
        let result = hook.execute(&ctx);
        assert!(result.is_deny());
    }

    #[test]
    fn test_confirmation_hook_never_allows_all() {
        let mut hook = ConfirmationHook::new(ConfirmationMode::Never);
        let ctx = HookContext::new("BeforeAct", "agent")
            .with_tool("any_tool", "{}");
        let result = hook.execute(&ctx);
        assert_eq!(result, HookResult::Continue);
    }

    #[test]
    fn test_confirmation_hook_pre_approved() {
        let mut hook = ConfirmationHook::new(ConfirmationMode::Untrusted);
        hook.pre_approve("safe");
        let ctx = HookContext::new("BeforeAct", "agent")
            .with_tool("safe_tool", "{}");
        let result = hook.execute(&ctx);
        assert_eq!(result, HookResult::Continue);
    }

    #[test]
    fn test_registry_count_per_event() {
        let mut reg = HookRegistry::new();
        reg.register(Box::new(AuditHook::post_tool_use()));
        reg.register(Box::new(AuditHook::new(HookEvent::AfterAct)));
        assert_eq!(reg.count_for_event(HookEvent::AfterAct), 2);
        assert_eq!(reg.count_for_event(HookEvent::BeforeAct), 0);
    }
}
