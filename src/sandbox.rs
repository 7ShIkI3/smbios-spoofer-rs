//! # Sandbox Manager — Isolation et Sécurité
//!
//! Inspiré de :
//! - **OpenAI Codex** : `SandboxManager` — read-only, workspace-write, danger-full-access
//! - **Claude Code** : permission system — allow/deny/ask per tool
//! - **Agent Zero** : Docker isolation avec `DeferredTask` threads
//! - **OpenClaw** : autonomy modes — readonly, supervised, full
//!
//! ## Niveaux d'isolation
//!
//! | Niveau      | Source       | Lecture | Écriture | Réseau | Approbation |
//! |-------------|-------------|---------|----------|--------|-------------|
//! | **ReadOnly**| Codex       | ✅      | ❌       | ❌     | Aucune      |
//! | **Sandbox** | Codex       | ✅      | Workspace| ❌     | Auto        |
//! | **Supervised** | OpenClaw | ✅      | ✅       | ✅     | Par action  |
//! | **FullAccess** | Codex     | ✅      | ✅       | ✅     | Aucune      |
//!
//! ## Permission Model (inspiré de Claude Code)
//!
//! ```text
//! allow: [Bash(git status), Bash(git log *)]
//! deny:  [Bash(rm -rf *), Bash(drop table *)]
//! ask:   [Bash(curl *), Bash(npm publish *)]
//! ```
//!
//! ## Dans le contexte UEFI
//!
//! En UEFI bare-metal, le "sandbox" est conceptuel :
//! - ReadOnly → on ne patch pas les tables, on lit seulement
//! - Sandbox → on patch mais avec validation préalable
//! - Supervised → chaque patch demande approbation
//! - FullAccess → patch direct sans restriction (mode actuel)

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;

// ─── Isolation Levels ───────────────────────────────────────────────────────

/// Niveau d'isolation du sandbox.
///
/// Mapping direct depuis Codex `SandboxType` :
/// ```text
/// codex: None → UEFI: FullAccess
/// codex: read-only → UEFI: ReadOnly
/// codex: workspace-write → UEFI: Sandbox
/// codex: danger-full-access → UEFI: FullAccess (mais avec audit)
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IsolationLevel {
    /// Lecture seule — observer sans modifier
    ReadOnly = 0,
    /// Sandbox — modifications limitées au scope autorisé
    Sandbox = 1,
    /// Supervisé — chaque action nécessite approbation
    Supervised = 2,
    /// Accès complet — pas de restrictions
    FullAccess = 3,
}

impl IsolationLevel {
    pub fn can_read(&self) -> bool {
        true // Tous les niveaux peuvent lire
    }

    pub fn can_write(&self) -> bool {
        matches!(self, Self::Sandbox | Self::Supervised | Self::FullAccess)
    }

    pub fn can_network(&self) -> bool {
        matches!(self, Self::Supervised | Self::FullAccess)
    }

    pub fn requires_approval(&self) -> bool {
        matches!(self, Self::ReadOnly | Self::Supervised)
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::ReadOnly => "read-only",
            Self::Sandbox => "sandbox",
            Self::Supervised => "supervised",
            Self::FullAccess => "full-access",
        }
    }
}

// ─── Permission System ──────────────────────────────────────────────────────

/// Type de permission pour une action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    /// Action autorisée sans demande
    Allow,
    /// Action bloquée
    Deny,
    /// Action nécessite approbation
    Ask,
}

impl Permission {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allow)
    }

    pub fn is_denied(&self) -> bool {
        matches!(self, Self::Deny)
    }
}

/// Une règle de permission.
///
/// Pattern : Claude Code `settings.json` allow/deny lists.
/// Supporte le matching par motif (wildcard `*`).
#[derive(Debug, Clone)]
pub struct PermissionRule {
    /// Pattern à matcher (ex: "patch_type1", "save_to_*", "*")
    pub pattern: String,
    /// Permission associée
    pub permission: Permission,
    /// Priorité (0 = plus prioritaire)
    pub priority: u8,
}

impl PermissionRule {
    pub fn allow(pattern: &str) -> Self {
        Self {
            pattern: String::from(pattern),
            permission: Permission::Allow,
            priority: 100,
        }
    }

    pub fn deny(pattern: &str) -> Self {
        Self {
            pattern: String::from(pattern),
            permission: Permission::Deny,
            priority: 0,
        }
    }

    pub fn ask(pattern: &str) -> Self {
        Self {
            pattern: String::from(pattern),
            permission: Permission::Ask,
            priority: 50,
        }
    }

    /// Vérifie si une action matche ce pattern.
    pub fn matches(&self, action: &str) -> bool {
        if self.pattern == "*" {
            return true;
        }
        if self.pattern.ends_with('*') {
            let prefix = &self.pattern[..self.pattern.len() - 1];
            return action.starts_with(prefix);
        }
        action == self.pattern
    }
}

/// Système de permissions — contrôle d'accès aux outils.
///
/// Pattern : Claude Code permission system + Codex approval_policy.
///
/// Règles évaluées par priorité (0 = la plus prioritaire).
/// La première règle qui matche l'emporte.
pub struct PermissionEngine {
    rules: Vec<PermissionRule>,
    default_permission: Permission,
    audit_enabled: bool,
}

impl PermissionEngine {
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            default_permission: Permission::Ask,
            audit_enabled: true,
        }
    }

    /// Ajoute une règle de permission.
    pub fn add_rule(&mut self, rule: PermissionRule) {
        self.rules.push(rule);
        self.rules.sort_by_key(|r| r.priority);
    }

    /// Autorise une action spécifique.
    pub fn allow(&mut self, pattern: &str) {
        self.add_rule(PermissionRule::allow(pattern));
    }

    /// Bloque une action spécifique.
    pub fn deny(&mut self, pattern: &str) {
        self.add_rule(PermissionRule::deny(pattern));
    }

    /// Demande approbation pour une action spécifique.
    pub fn ask(&mut self, pattern: &str) {
        self.add_rule(PermissionRule::ask(pattern));
    }

    /// Évalue la permission pour une action donnée.
    ///
    /// Retourne la première règle qui matche, ou la permission par défaut.
    pub fn evaluate(&self, action: &str) -> Permission {
        for rule in &self.rules {
            if rule.matches(action) {
                return rule.permission;
            }
        }
        self.default_permission
    }

    /// Vérifie si une action est autorisée.
    pub fn is_allowed(&self, action: &str) -> bool {
        self.evaluate(action).is_allowed()
    }

    /// Vérifie si une action est bloquée.
    pub fn is_denied(&self, action: &str) -> bool {
        self.evaluate(action).is_denied()
    }

    /// Change la permission par défaut.
    pub fn set_default(&mut self, permission: Permission) {
        self.default_permission = permission;
    }

    /// Active/désactive l'audit.
    pub fn set_audit(&mut self, enabled: bool) {
        self.audit_enabled = enabled;
    }

    /// Liste toutes les règles.
    pub fn list_rules(&self) -> &[PermissionRule] {
        &self.rules
    }

    /// Compte le nombre de règles.
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Résumé texte des règles.
    pub fn summary(&self) -> String {
        let allow_count = self.rules.iter().filter(|r| r.permission == Permission::Allow).count();
        let deny_count = self.rules.iter().filter(|r| r.permission == Permission::Deny).count();
        let ask_count = self.rules.iter().filter(|r| r.permission == Permission::Ask).count();
        format!(
            "Permissions: {} allow, {} deny, {} ask (default: {:?})",
            allow_count, deny_count, ask_count, self.default_permission
        )
    }
}

impl Default for PermissionEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Sandbox Manager ────────────────────────────────────────────────────────

/// Contexte d'exécution d'une action dans le sandbox.
#[derive(Debug, Clone)]
pub struct SandboxContext {
    pub action: String,
    pub parameters: String,
    pub isolation_level: IsolationLevel,
    pub timestamp: u64,
}

impl SandboxContext {
    pub fn new(action: &str, level: IsolationLevel) -> Self {
        Self {
            action: String::from(action),
            parameters: String::new(),
            isolation_level: level,
            timestamp: 0,
        }
    }

    pub fn with_params(mut self, params: &str) -> Self {
        self.parameters = String::from(params);
        self
    }
}

/// Résultat d'une vérification sandbox.
#[derive(Debug, Clone)]
pub enum SandboxDecision {
    /// Action autorisée
    Allowed,
    /// Action bloquée avec raison
    Denied { reason: String },
    /// Action nécessite approbation
    ApprovalRequired { action: String, risk_level: RiskLevel },
}

/// Niveau de risque d'une action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

impl RiskLevel {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }

    pub fn score(&self) -> u8 {
        match self {
            Self::Low => 25,
            Self::Medium => 50,
            Self::High => 75,
            Self::Critical => 100,
        }
    }
}

/// Sandbox Manager — décide si une action peut s'exécuter.
///
/// Pattern : Codex `ToolOrchestrator::run()` — approval → sandbox → attempt → retry.
///
/// La décision est basée sur :
/// 1. Le niveau d'isolation configuré
/// 2. Le moteur de permissions (allow/deny/ask lists)
/// 3. Le niveau de risque de l'action
pub struct SandboxManager {
    isolation_level: IsolationLevel,
    permissions: PermissionEngine,
    risk_threshold: u8, // score max avant de demander approbation
}

impl SandboxManager {
    pub fn new(level: IsolationLevel) -> Self {
        Self {
            isolation_level: level,
            permissions: PermissionEngine::new(),
            risk_threshold: 50,
        }
    }

    /// Configure le niveau d'isolation.
    pub fn set_isolation(&mut self, level: IsolationLevel) {
        self.isolation_level = level;
    }

    /// Définit le seuil de risque.
    pub fn set_risk_threshold(&mut self, threshold: u8) {
        self.risk_threshold = threshold;
    }

    /// Accès au moteur de permissions pour ajouter des règles.
    pub fn permissions_mut(&mut self) -> &mut PermissionEngine {
        &mut self.permissions
    }

    /// Évalue si une action peut s'exécuter.
    ///
    /// Ordre d'évaluation (inspiré de Codex):
    /// 1. Permission engine (allow/deny explicite)
    /// 2. Niveau d'isolation (read-only bloque les writes)
    /// 3. Niveau de risque (au-dessus du seuil → approbation)
    pub fn evaluate(&self, ctx: &SandboxContext) -> SandboxDecision {
        // 1. Vérifier le niveau d'isolation (ReadOnly bloque toujours les écritures)
        if !self.isolation_level.can_write() && self.is_destructive(ctx) {
            return SandboxDecision::Denied {
                reason: format!(
                    "Write operation '{}' blocked at isolation level '{}'",
                    ctx.action,
                    self.isolation_level.as_str()
                ),
            };
        }

        // 2. Vérifier les permissions explicites
        match self.permissions.evaluate(&ctx.action) {
            Permission::Deny => {
                return SandboxDecision::Denied {
                    reason: format!(
                        "Action '{}' is explicitly denied by permission rules",
                        ctx.action
                    ),
                };
            }
            Permission::Ask => {
                return SandboxDecision::ApprovalRequired {
                    action: ctx.action.clone(),
                    risk_level: self.assess_risk(ctx),
                };
            }
            Permission::Allow => {
                // Continue la vérification
            }
        }

        // 3. Vérifier le niveau de risque
        let risk = self.assess_risk(ctx);
        if risk.score() > self.risk_threshold {
            return SandboxDecision::ApprovalRequired {
                action: ctx.action.clone(),
                risk_level: risk,
            };
        }

        SandboxDecision::Allowed
    }

    /// Évalue le niveau de risque d'une action.
    fn assess_risk(&self, ctx: &SandboxContext) -> RiskLevel {
        let action = &ctx.action.to_lowercase();

        // Actions destructives → risque critique
        let critical_patterns = [
            "format", "delete", "erase", "wipe", "destroy",
            "overwrite_boot", "patch_bootloader",
        ];
        if critical_patterns.iter().any(|p| action.contains(p)) {
            return RiskLevel::Critical;
        }

        // Écriture mémoire → risque élevé
        let high_patterns = [
            "patch", "write", "modify", "edit", "replace",
            "save_to_nvram", "save_to_disk",
        ];
        if high_patterns.iter().any(|p| action.contains(p)) {
            return RiskLevel::High;
        }

        // Lecture/scan → risque moyen
        let medium_patterns = [
            "find", "search", "scan", "read", "load",
            "discover",
        ];
        if medium_patterns.iter().any(|p| action.contains(p)) {
            return RiskLevel::Medium;
        }

        RiskLevel::Low
    }

    /// Détermine si une action est destructive (modifie l'état).
    fn is_destructive(&self, ctx: &SandboxContext) -> bool {
        let action = &ctx.action.to_lowercase();
        let destructive = [
            "patch", "write", "edit", "modify", "delete",
            "save", "persist", "format", "erase",
        ];
        destructive.iter().any(|p| action.contains(p))
    }

    /// Vérifie rapidement si une action est autorisée.
    pub fn quick_check(&self, action: &str, level: IsolationLevel) -> bool {
        let ctx = SandboxContext::new(action, level);
        matches!(self.evaluate(&ctx), SandboxDecision::Allowed)
    }

    /// Résumé de la configuration.
    pub fn summary(&self) -> String {
        format!(
            "Sandbox: level={}, risk_threshold={}, {}",
            self.isolation_level.as_str(),
            self.risk_threshold,
            self.permissions.summary()
        )
    }
}

// ─── Approval Manager ───────────────────────────────────────────────────────

/// État d'une demande d'approbation.
#[derive(Debug, Clone)]
pub enum ApprovalState {
    Pending,
    Approved { by: String, reason: String },
    Denied { by: String, reason: String },
    TimedOut,
}

/// Une demande d'approbation en attente.
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    pub id: u32,
    pub action: String,
    pub risk_level: RiskLevel,
    pub context: String,
    pub state: ApprovalState,
    pub created_at: u64,
    pub timeout_ms: u64,
}

impl ApprovalRequest {
    pub fn new(id: u32, action: &str, risk: RiskLevel, context: &str, timeout_ms: u64) -> Self {
        Self {
            id,
            action: String::from(action),
            risk_level: risk,
            context: String::from(context),
            state: ApprovalState::Pending,
            created_at: 0,
            timeout_ms,
        }
    }

    pub fn is_pending(&self) -> bool {
        matches!(self.state, ApprovalState::Pending)
    }

    pub fn is_approved(&self) -> bool {
        matches!(self.state, ApprovalState::Approved { .. })
    }

    pub fn approve(&mut self, by: &str, reason: &str) {
        self.state = ApprovalState::Approved {
            by: String::from(by),
            reason: String::from(reason),
        };
    }

    pub fn deny(&mut self, by: &str, reason: &str) {
        self.state = ApprovalState::Denied {
            by: String::from(by),
            reason: String::from(reason),
        };
    }

    pub fn timeout(&mut self) {
        self.state = ApprovalState::TimedOut;
    }
}

/// Approval Manager — gère les demandes d'approbation.
///
/// Pattern : Codex `GuardianReviewer` + Claude Code "PermissionRequest hooks".
///
/// File d'attente de demandes d'approbation. Chaque demande a :
/// - Un timeout (expire automatiquement → deny)
/// - Un niveau de risque (guide la décision)
/// - Un contexte (pour que l'humain/guardian comprenne)
pub struct ApprovalManager {
    requests: Vec<ApprovalRequest>,
    next_id: u32,
    auto_approve_below: u8, // score de risque en dessous duquel on auto-approuve
    default_timeout_ms: u64,
}

impl ApprovalManager {
    pub fn new() -> Self {
        Self {
            requests: Vec::new(),
            next_id: 1,
            auto_approve_below: 25,
            default_timeout_ms: 30000, // 30 secondes
        }
    }

    /// Crée une demande d'approbation.
    pub fn request(
        &mut self,
        action: &str,
        risk: RiskLevel,
        context: &str,
    ) -> u32 {
        let id = self.next_id;
        self.next_id += 1;

        // Auto-approve si le risque est très bas
        if risk.score() <= self.auto_approve_below {
            let mut req = ApprovalRequest::new(id, action, risk, context, 0);
            req.approve("auto", "risk below threshold");
            self.requests.push(req);
            return id;
        }

        self.requests.push(ApprovalRequest::new(
            id,
            action,
            risk,
            context,
            self.default_timeout_ms,
        ));
        id
    }

    /// Approuve une demande par ID.
    pub fn approve(&mut self, id: u32, by: &str, reason: &str) -> bool {
        if let Some(req) = self.requests.iter_mut().find(|r| r.id == id) {
            req.approve(by, reason);
            true
        } else {
            false
        }
    }

    /// Refuse une demande par ID.
    pub fn deny(&mut self, id: u32, by: &str, reason: &str) -> bool {
        if let Some(req) = self.requests.iter_mut().find(|r| r.id == id) {
            req.deny(by, reason);
            true
        } else {
            false
        }
    }

    /// Récupère une demande par ID.
    pub fn get(&self, id: u32) -> Option<&ApprovalRequest> {
        self.requests.iter().find(|r| r.id == id)
    }

    /// Liste les demandes en attente.
    pub fn pending_requests(&self) -> Vec<&ApprovalRequest> {
        self.requests
            .iter()
            .filter(|r| r.is_pending())
            .collect()
    }

    /// Nombre de demandes en attente.
    pub fn pending_count(&self) -> usize {
        self.requests.iter().filter(|r| r.is_pending()).count()
    }

    /// Nettoie les demandes résolues (approuvées/refusées).
    pub fn cleanup(&mut self) {
        self.requests.retain(|r| r.is_pending());
    }

    /// Configure le seuil d'auto-approbation.
    pub fn set_auto_approve(&mut self, risk_score: u8) {
        self.auto_approve_below = risk_score;
    }

    /// Résumé des approbations.
    pub fn summary(&self) -> String {
        let pending = self.pending_count();
        let total = self.requests.len();
        let approved = self.requests.iter().filter(|r| r.is_approved()).count();
        let denied = self.requests.iter().filter(|r| matches!(r.state, ApprovalState::Denied { .. })).count();
        format!(
            "Approvals: {} pending, {} approved, {} denied ({} total)",
            pending, approved, denied, total
        )
    }
}

impl Default for ApprovalManager {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── IsolationLevel ──────────────────────────────────────────────────

    #[test]
    fn test_isolation_level_capabilities() {
        assert!(IsolationLevel::ReadOnly.can_read());
        assert!(!IsolationLevel::ReadOnly.can_write());
        assert!(!IsolationLevel::ReadOnly.can_network());

        assert!(IsolationLevel::Sandbox.can_write());
        assert!(!IsolationLevel::Sandbox.can_network());

        assert!(IsolationLevel::FullAccess.can_write());
        assert!(IsolationLevel::FullAccess.can_network());
    }

    #[test]
    fn test_isolation_level_ordering() {
        assert!(IsolationLevel::FullAccess > IsolationLevel::Sandbox);
        assert!(IsolationLevel::Sandbox > IsolationLevel::ReadOnly);
    }

    // ── PermissionEngine ────────────────────────────────────────────────

    #[test]
    fn test_permission_engine_default_ask() {
        let engine = PermissionEngine::new();
        assert_eq!(engine.evaluate("unknown_action"), Permission::Ask);
    }

    #[test]
    fn test_permission_engine_allow() {
        let mut engine = PermissionEngine::new();
        engine.allow("safe_action");
        assert_eq!(engine.evaluate("safe_action"), Permission::Allow);
    }

    #[test]
    fn test_permission_engine_deny() {
        let mut engine = PermissionEngine::new();
        engine.deny("dangerous_action");
        assert_eq!(engine.evaluate("dangerous_action"), Permission::Deny);
        assert!(engine.is_denied("dangerous_action"));
    }

    #[test]
    fn test_permission_engine_wildcard() {
        let mut engine = PermissionEngine::new();
        engine.allow("patch_*");
        assert!(engine.is_allowed("patch_type1"));
        assert!(engine.is_allowed("patch_type2"));
        assert_eq!(engine.evaluate("read_table"), Permission::Ask);
    }

    #[test]
    fn test_permission_engine_priority() {
        let mut engine = PermissionEngine::new();
        // Deny a priorité sur Allow (priority 0 vs 100)
        engine.deny("patch_type1");
        engine.allow("patch_*");
        assert!(engine.is_denied("patch_type1"));
        assert!(engine.is_allowed("patch_type2"));
    }

    #[test]
    fn test_permission_engine_set_default() {
        let mut engine = PermissionEngine::new();
        engine.set_default(Permission::Deny);
        assert_eq!(engine.evaluate("anything"), Permission::Deny);
    }

    // ── SandboxManager ──────────────────────────────────────────────────

    #[test]
    fn test_sandbox_readonly_blocks_writes() {
        let manager = SandboxManager::new(IsolationLevel::ReadOnly);
        let ctx = SandboxContext::new("patch_type1", IsolationLevel::ReadOnly);
        match manager.evaluate(&ctx) {
            SandboxDecision::Denied { .. } => {} // OK
            other => panic!("Expected Denied, got {:?}", other),
        }
    }

    #[test]
    fn test_sandbox_fullaccess_allows() {
        let mut manager = SandboxManager::new(IsolationLevel::FullAccess);
        manager.permissions_mut().set_default(Permission::Allow);
        let ctx = SandboxContext::new("read_table", IsolationLevel::FullAccess);
        match manager.evaluate(&ctx) {
            SandboxDecision::Allowed => {} // OK
            other => panic!("Expected Allowed, got {:?}", other),
        }
    }

    #[test]
    fn test_sandbox_high_risk_requires_approval() {
        let mut manager = SandboxManager::new(IsolationLevel::FullAccess);
        manager.permissions_mut().set_default(Permission::Allow);
        manager.set_risk_threshold(30);
        // "patch" est HIGH risk (score 75 > 30)
        let ctx = SandboxContext::new("patch_bootloader", IsolationLevel::FullAccess);
        match manager.evaluate(&ctx) {
            SandboxDecision::ApprovalRequired { .. } => {} // OK
            other => panic!("Expected ApprovalRequired, got {:?}", other),
        }
    }

    // ── ApprovalManager ─────────────────────────────────────────────────

    #[test]
    fn test_approval_manager_request() {
        let mut am = ApprovalManager::new();
        let id = am.request("patch", RiskLevel::High, "Modifying SMBIOS tables");
        assert_eq!(am.pending_count(), 1);
        assert!(am.get(id).is_some());
    }

    #[test]
    fn test_approval_manager_auto_approve_low_risk() {
        let mut am = ApprovalManager::new();
        let id = am.request("read", RiskLevel::Low, "Reading table");
        let req = am.get(id).unwrap();
        assert!(req.is_approved(), "Low risk should be auto-approved");
    }

    #[test]
    fn test_approval_manager_approve_deny() {
        let mut am = ApprovalManager::new();
        let id = am.request("patch", RiskLevel::High, "context");
        assert!(am.approve(id, "user", "looks safe"));
        assert!(am.get(id).unwrap().is_approved());
        assert_eq!(am.pending_count(), 0);
    }

    #[test]
    fn test_approval_manager_cleanup() {
        let mut am = ApprovalManager::new();
        am.request("action1", RiskLevel::High, "ctx");
        am.request("action2", RiskLevel::High, "ctx");
        am.approve(1, "user", "ok");
        am.deny(2, "user", "no");
        am.cleanup();
        assert_eq!(am.pending_count(), 0);
    }

    // ── PermissionRule ──────────────────────────────────────────────────

    #[test]
    fn test_permission_rule_wildcard_match() {
        let rule = PermissionRule::allow("save_*");
        assert!(rule.matches("save_to_nvram"));
        assert!(rule.matches("save_to_disk"));
        assert!(!rule.matches("load_config"));
    }

    #[test]
    fn test_permission_rule_exact_match() {
        let rule = PermissionRule::deny("format_disk");
        assert!(rule.matches("format_disk"));
        assert!(!rule.matches("format_partition"));
    }

    // ── RiskLevel ───────────────────────────────────────────────────────

    #[test]
    fn test_risk_level_scores() {
        assert!(RiskLevel::Critical.score() > RiskLevel::High.score());
        assert!(RiskLevel::High.score() > RiskLevel::Medium.score());
        assert!(RiskLevel::Medium.score() > RiskLevel::Low.score());
    }
}
