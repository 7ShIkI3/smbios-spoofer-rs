//! # Reflection Engine — Auto-critique et Amélioration Continue
//!
//! Inspiré de :
//! - **multi_ai_agent** (vishal48-tech) : Reflexion Agent qui apprend de ses erreurs
//! - **gaia-agent** (kvpratama) : ReAct + Reflection step
//! - **OVOS agentic-loop** : ReflexionEngine + TreeOfThoughtsEngine
//! - **all-agentic-architectures** (FareedKhan-dev) : Reflection, Tree-of-Thoughts, Self-Improvement
//! - **Agent Zero** : behavior_adjustment tool (auto-modification en runtime)
//!
//! ## Patterns implémentés
//!
//! | Pattern               | Source         | Description                                      |
//! |-----------------------|----------------|--------------------------------------------------|
//! | **ReflectionLoop**    | multi_ai_agent | Generate → Critique → Refine                     |
//! | **TreeOfThoughts**    | OVOS, Fareed   | Expand N branches → Evaluate → Prune → Select    |
//! | **SelfImprovement**   | Agent Zero     | Mesure → Compare → Adjust behavior               |
//! | **ErrorRecovery**    | Codex          | On failure → Diagnose → Recovery plan → Retry    |
//!
//! ## Comment ça s'intègre au spoofer SMBIOS
//!
//! ```text
//! 1. Generate UUID/serials    → ReflectionLoop évalue la qualité
//! 2. Patch SMBIOS tables       → ErrorRecovery gère les échecs mémoire
//! 3. Persist config            → SelfImprovement ajuste les paramètres
//! 4. Next boot                 → TreeOfThoughts explore des alternatives
//! ```

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use crate::agentic::AuditTrail;

// ─── Reflection Loop ───────────────────────────────────────────────────────

/// Résultat d'une étape de réflexion.
#[derive(Debug, Clone)]
pub struct ReflectionResult {
    /// L'action effectuée
    pub action: String,
    /// Le résultat brut
    pub outcome: String,
    /// Score de succès (0.0 - 1.0)
    pub success_score: f32,
    /// Ce qui a bien fonctionné
    pub strengths: Vec<String>,
    /// Ce qui doit être amélioré
    pub weaknesses: Vec<String>,
    /// Suggestions pour le prochain essai
    pub suggestions: Vec<String>,
    /// Nombre de tentatives
    pub attempts: usize,
}

impl ReflectionResult {
    pub fn new(action: &str, outcome: &str) -> Self {
        Self {
            action: String::from(action),
            outcome: String::from(outcome),
            success_score: 0.0,
            strengths: Vec::new(),
            weaknesses: Vec::new(),
            suggestions: Vec::new(),
            attempts: 0,
        }
    }

    pub fn is_perfect(&self) -> bool {
        self.success_score >= 0.99
    }

    pub fn needs_improvement(&self) -> bool {
        self.success_score < 0.8
    }
}

/// Trait pour une action réflexive — quelque chose qu'on peut
/// exécuter, évaluer, et améliorer par itération.
pub trait ReflectiveAction {
    /// Nom de l'action.
    fn name(&self) -> &str;

    /// Exécute l'action et retourne le résultat brut.
    fn execute(&mut self) -> String;

    /// Évalue la qualité du résultat.
    /// Retourne un score entre 0.0 (échec total) et 1.0 (parfait).
    fn evaluate(&self, result: &str) -> f32;

    /// Génère des suggestions d'amélioration basées sur le résultat.
    fn critique(&self, result: &str, score: f32) -> Vec<String>;

    /// Applique les suggestions et réessaie.
    fn refine(&mut self, suggestions: &[String]);

    /// Vérifie si on doit continuer à itérer.
    fn should_continue(&self, result: &ReflectionResult, max_iterations: usize) -> bool {
        result.attempts < max_iterations && !result.is_perfect()
    }
}

/// ReflectionLoop : le pattern Generate → Critique → Refine.
///
/// Inspiré de `ReflexionAgent` de multi_ai_agent et `Reflection` de gaia-agent.
/// La boucle :
/// ```text
/// while !perfect && iterations < max:
///     result = action.execute()
///     score = action.evaluate(result)
///     suggestions = action.critique(result, score)
///     if score < threshold:
///         action.refine(suggestions)
///         retry
///     else:
///         break
/// ```
pub struct ReflectionLoop {
    max_iterations: usize,
    quality_threshold: f32,
    audit: AuditTrail,
}

impl ReflectionLoop {
    pub fn new(max_iterations: usize, quality_threshold: f32) -> Self {
        Self {
            max_iterations,
            quality_threshold,
            audit: AuditTrail::new(),
        }
    }

    /// Exécute la boucle réflexive sur une action.
    ///
    /// Continue jusqu'à ce que :
    /// - Le score dépasse le seuil de qualité
    /// - Le nombre max d'itérations est atteint
    /// - L'action indique qu'elle ne peut plus s'améliorer
    pub fn run(&mut self, action: &mut dyn ReflectiveAction) -> ReflectionResult {
        let mut result = ReflectionResult::new(action.name(), "");
        let action_name = String::from(action.name());

        while result.attempts < self.max_iterations {
            // ── Generate ──────────────────────────────────────────────
            let outcome = action.execute();
            result.outcome = outcome;

            // ── Evaluate ─────────────────────────────────────────────
            let score = action.evaluate(&result.outcome);
            result.success_score = score;

            // ── Critique ─────────────────────────────────────────────
            let suggestions = action.critique(&result.outcome, score);
            result.weaknesses = suggestions.clone();

            self.audit.record(
                crate::agentic::AuditLevel::Info,
                &action_name,
                "reflect_iteration",
                &format!(
                    "Iteration {}: score={:.2}, {} suggestions",
                    result.attempts,
                    score,
                    suggestions.len()
                ),
            );

            // ── Check termination ────────────────────────────────────
            if score >= self.quality_threshold || score >= 0.99 {
                result.strengths.push(format!(
                    "Achieved target quality ({} >= {})",
                    score, self.quality_threshold
                ));
                break;
            }

            if !action.should_continue(&result, self.max_iterations) {
                break;
            }

            // ── Refine ───────────────────────────────────────────────
            action.refine(&suggestions);
            result.attempts += 1;
            result.suggestions = suggestions;
        }

        result
    }
}

// ─── Tree of Thoughts ──────────────────────────────────────────────────────

/// Un nœud dans l'arbre de pensée.
#[derive(Debug, Clone)]
pub struct ThoughtNode {
    /// Identifiant unique du nœud
    pub id: usize,
    /// Contenu de la pensée (texte descriptif)
    pub content: String,
    /// Score d'évaluation (0.0 - 1.0)
    pub score: f32,
    /// Profondeur dans l'arbre
    pub depth: usize,
    /// Nœud parent (None pour la racine)
    pub parent_id: Option<usize>,
    /// Enfants de ce nœud
    pub children: Vec<usize>,
}

impl ThoughtNode {
    pub fn new(id: usize, content: &str, depth: usize, parent_id: Option<usize>) -> Self {
        Self {
            id,
            content: String::from(content),
            score: 0.0,
            depth,
            parent_id,
            children: Vec::new(),
        }
    }

    pub fn is_leaf(&self) -> bool {
        self.children.is_empty()
    }
}

/// Fonction d'expansion : génère des pensées alternatives à partir d'un nœud.
pub type ExpandFn = dyn Fn(&ThoughtNode) -> Vec<String>;

/// Fonction d'évaluation : note une pensée (0.0 - 1.0).
pub type EvaluateFn = dyn Fn(&str) -> f32;

/// Tree of Thoughts — exploration arborescente de solutions.
///
/// Pattern de **OVOS TreeOfThoughtsEngine** et **all-agentic-architectures**.
/// Algorithme BFS avec élagage (beam search) :
///
/// ```text
/// initialize root thought
/// for depth in 0..max_depth:
///     for each active thought at this depth:
///         generate N children (expand)
///         score each child (evaluate)
///     keep top-K children (prune)
///     if any child score > threshold:
///         return best path
/// select best leaf
/// ```
pub struct TreeOfThoughts {
    nodes: Vec<ThoughtNode>,
    next_id: usize,
    beam_width: usize,      // K = nombre de nœuds gardés par niveau
    max_depth: usize,
    score_threshold: f32,
}

impl TreeOfThoughts {
    pub fn new(beam_width: usize, max_depth: usize, score_threshold: f32) -> Self {
        Self {
            nodes: Vec::new(),
            next_id: 0,
            beam_width,
            max_depth,
            score_threshold,
        }
    }

    /// Crée le nœud racine.
    pub fn root(&mut self, content: &str) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        self.nodes.push(ThoughtNode::new(id, content, 0, None));
        id
    }

    /// Explore l'arbre de pensée.
    ///
    /// `expand` — génère de nouvelles pensées à partir d'un nœud existant.
    /// `evaluate` — évalue la qualité d'une pensée.
    ///
    /// Retourne le chemin du meilleur nœud (de la racine à la feuille).
    pub fn search(
        &mut self,
        expand: &ExpandFn,
        evaluate: &EvaluateFn,
    ) -> Vec<ThoughtNode> {
        let mut active_ids: Vec<usize> = vec![0]; // commence à la racine

        for _depth in 0..self.max_depth {
            let mut candidates: Vec<(usize, f32)> = Vec::new();

            // ── Expand ───────────────────────────────────────────────
            for &node_id in &active_ids {
                // Copier les infos nécessaires avant d'emprunter mutablement
                let depth = self.nodes[node_id].depth;
                let children = expand(&self.nodes[node_id]);

                for child_content in children {
                    let child_id = self.next_id;
                    self.next_id += 1;

                    // Évaluer l'enfant
                    let score = evaluate(&child_content);

                    let mut child = ThoughtNode::new(
                        child_id,
                        &child_content,
                        depth + 1,
                        Some(node_id),
                    );
                    child.score = score;
                    self.nodes.push(child);

                    // Ajouter aux enfants du parent
                    if let Some(parent) = self.nodes.get_mut(node_id) {
                        parent.children.push(child_id);
                    }

                    candidates.push((child_id, score));

                    // Early exit si on trouve une solution excellente
                    if score >= self.score_threshold {
                        break;
                    }
                }
            }

            if candidates.is_empty() {
                break;
            }

            // ── Prune (beam search) ──────────────────────────────────
            candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));
            candidates.truncate(self.beam_width);

            active_ids = candidates.iter().map(|(id, _)| *id).collect();

            // Vérifier si on a atteint le seuil
            if candidates.first().map(|(_, s)| *s).unwrap_or(0.0) >= self.score_threshold {
                break;
            }
        }

        // ── Sélectionner le meilleur chemin ─────────────────────────
        self.best_path()
    }

    /// Reconstruit le chemin du meilleur nœud feuille vers la racine.
    fn best_path(&self) -> Vec<ThoughtNode> {
        if self.nodes.is_empty() {
            return Vec::new();
        }

        // Trouver le nœud avec le meilleur score
        let mut best_idx = 0;
        let mut best_score = 0.0f32;
        for (i, node) in self.nodes.iter().enumerate() {
            if node.score > best_score {
                best_score = node.score;
                best_idx = i;
            }
        }

        // Remonter au parent
        let mut path = Vec::new();
        let mut current_id = Some(best_idx);
        while let Some(id) = current_id {
            if id < self.nodes.len() {
                path.push(self.nodes[id].clone());
                current_id = self.nodes[id].parent_id;
            } else {
                break;
            }
        }
        path.reverse();
        path
    }

    /// Nombre total de nœuds dans l'arbre.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Profondeur maximale atteinte.
    pub fn max_depth_reached(&self) -> usize {
        self.nodes.iter().map(|n| n.depth).max().unwrap_or(0)
    }

    /// Meilleur score trouvé.
    pub fn best_score(&self) -> f32 {
        self.nodes
            .iter()
            .map(|n| n.score)
            .max_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal))
            .unwrap_or(0.0)
    }
}

// ─── Error Recovery ────────────────────────────────────────────────────────

/// Stratégie de recovery après une erreur.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryStrategy {
    /// Réessayer avec les mêmes paramètres
    Retry,
    /// Réessayer avec des paramètres modifiés
    RetryWithChanges,
    /// Utiliser une approche alternative
    Fallback,
    /// Abandonner cette opération
    Abort,
    /// Ignorer l'erreur et continuer
    Skip,
}

/// Contexte d'une erreur pour le recovery.
#[derive(Debug, Clone)]
pub struct ErrorContext {
    pub operation: String,
    pub error_message: String,
    pub attempt: usize,
    pub max_attempts: usize,
    pub is_fatal: bool,
}

impl ErrorContext {
    pub fn new(operation: &str, error: &str, max_attempts: usize) -> Self {
        Self {
            operation: String::from(operation),
            error_message: String::from(error),
            attempt: 0,
            max_attempts,
            is_fatal: false,
        }
    }

    pub fn can_retry(&self) -> bool {
        self.attempt < self.max_attempts && !self.is_fatal
    }
}

/// Error Recovery Engine — décide quoi faire après une erreur.
///
/// Pattern : Codex `ErrorRecovery.getRecoveryPlan()` et Agent Zero retry logic.
///
/// Stratégies de décision :
/// - Erreurs transitoires → Retry avec backoff exponentiel
/// - Erreurs de validation → RetryWithChanges (modifier les params)
/// - Erreurs fatales → Abort ou Skip selon la config
pub struct ErrorRecovery {
    max_retries: usize,
    base_delay_ms: u64,
    max_delay_ms: u64,
    fallback_actions: Vec<(String, String)>, // (erreur_pattern, fallback_action)
}

impl ErrorRecovery {
    pub fn new(max_retries: usize) -> Self {
        Self {
            max_retries,
            base_delay_ms: 100,
            max_delay_ms: 5000,
            fallback_actions: Vec::new(),
        }
    }

    /// Ajoute un fallback pour un type d'erreur spécifique.
    pub fn add_fallback(&mut self, error_pattern: &str, fallback_action: &str) {
        self.fallback_actions
            .push((String::from(error_pattern), String::from(fallback_action)));
    }

    /// Décide de la stratégie de recovery pour une erreur donnée.
    pub fn decide(&self, ctx: &ErrorContext) -> RecoveryStrategy {
        // Erreur fatale → abort
        if ctx.is_fatal {
            return RecoveryStrategy::Abort;
        }

        // Plus de retries → abort
        if !ctx.can_retry() || ctx.attempt >= self.max_retries {
            // Vérifier si on a un fallback
            for (pattern, _action) in &self.fallback_actions {
                if ctx.error_message.contains(pattern.as_str()) {
                    return RecoveryStrategy::Fallback;
                }
            }
            return RecoveryStrategy::Abort;
        }

        // Erreur transitoire → retry
        let transient_patterns = [
            "timeout", "connection", "busy", "temporary", "retry",
            "unavailable", "throttle", "rate limit",
        ];
        let is_transient = transient_patterns
            .iter()
            .any(|p| ctx.error_message.to_lowercase().contains(p));

        if is_transient {
            return RecoveryStrategy::Retry;
        }

        // Erreur de validation ou de paramètre → retry avec changements
        let validation_patterns = [
            "invalid", "validation", "malformed", "out of range",
            "not found", "missing", "bad format",
        ];
        let is_validation = validation_patterns
            .iter()
            .any(|p| ctx.error_message.to_lowercase().contains(p));

        if is_validation {
            return RecoveryStrategy::RetryWithChanges;
        }

        // Défaut : retry simple
        RecoveryStrategy::Retry
    }

    /// Calcule le délai avant le prochain retry (backoff exponentiel avec jitter).
    pub fn retry_delay_ms(&self, attempt: usize) -> u64 {
        let delay = self.base_delay_ms * (1u64 << attempt.min(6)); // max 6.4s
        let with_jitter = delay + (delay / 4); // +25% jitter
        with_jitter.min(self.max_delay_ms)
    }

    /// Trouve l'action de fallback pour une erreur.
    pub fn get_fallback(&self, error_message: &str) -> Option<&str> {
        for (pattern, action) in &self.fallback_actions {
            if error_message.contains(pattern.as_str()) {
                return Some(action.as_str());
            }
        }
        None
    }
}

// ─── Self Improvement ──────────────────────────────────────────────────────

/// Métrique de performance suivie dans le temps.
#[derive(Debug, Clone)]
pub struct PerformanceMetric {
    pub name: String,
    pub values: Vec<f32>,
    pub window_size: usize,
}

impl PerformanceMetric {
    pub fn new(name: &str, window_size: usize) -> Self {
        Self {
            name: String::from(name),
            values: Vec::new(),
            window_size,
        }
    }

    pub fn record(&mut self, value: f32) {
        self.values.push(value);
        if self.values.len() > self.window_size {
            self.values.remove(0);
        }
    }

    /// Moyenne mobile sur la fenêtre.
    pub fn moving_average(&self) -> f32 {
        if self.values.is_empty() {
            return 0.0;
        }
        self.values.iter().sum::<f32>() / self.values.len() as f32
    }

    /// Tendance : positive = amélioration, négative = dégradation.
    pub fn trend(&self) -> f32 {
        if self.values.len() < 2 {
            return 0.0;
        }
        let half = self.values.len() / 2;
        let first_half: f32 = self.values[..half].iter().sum::<f32>() / half as f32;
        let second_half: f32 = self.values[half..].iter().sum::<f32>() / (self.values.len() - half) as f32;
        second_half - first_half
    }

    pub fn is_improving(&self) -> bool {
        self.trend() > 0.0
    }

    pub fn min(&self) -> f32 {
        self.values
            .iter()
            .min_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal))
            .copied()
            .unwrap_or(0.0)
    }

    pub fn max(&self) -> f32 {
        self.values
            .iter()
            .max_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal))
            .copied()
            .unwrap_or(0.0)
    }
}

/// SelfImprovementEngine — mesure et ajuste le comportement.
///
/// Pattern : Agent Zero `behavior_adjustment` tool.
/// Suit des métriques de performance et suggère des ajustements.
pub struct SelfImprovementEngine {
    metrics: Vec<PerformanceMetric>,
    improvement_threshold: f32,
}

impl SelfImprovementEngine {
    pub fn new() -> Self {
        Self {
            metrics: Vec::new(),
            improvement_threshold: 0.05, // 5% d'amélioration minimum
        }
    }

    /// Ajoute une métrique à suivre.
    pub fn track(&mut self, name: &str, window_size: usize) {
        self.metrics
            .push(PerformanceMetric::new(name, window_size));
    }

    /// Enregistre une valeur pour une métrique.
    pub fn record(&mut self, name: &str, value: f32) {
        for metric in self.metrics.iter_mut() {
            if metric.name == name {
                metric.record(value);
                return;
            }
        }
    }

    /// Évalue si le système s'améliore ou se dégrade.
    pub fn health_check(&self) -> HealthStatus {
        let mut improving = 0;
        let mut degrading = 0;
        let mut stable = 0;

        for metric in &self.metrics {
            let trend = metric.trend();
            if trend > self.improvement_threshold {
                improving += 1;
            } else if trend < -self.improvement_threshold {
                degrading += 1;
            } else {
                stable += 1;
            }
        }

        if improving > degrading && improving > stable {
            HealthStatus::Improving
        } else if degrading > improving {
            HealthStatus::Degrading
        } else {
            HealthStatus::Stable
        }
    }

    /// Retourne les métriques qui se dégradent.
    pub fn degraded_metrics(&self) -> Vec<&PerformanceMetric> {
        self.metrics
            .iter()
            .filter(|m| m.trend() < -self.improvement_threshold)
            .collect()
    }

    /// Suggère des actions d'amélioration basées sur les métriques.
    pub fn suggest_improvements(&self) -> Vec<String> {
        let mut suggestions = Vec::new();
        for metric in self.degraded_metrics() {
            suggestions.push(format!(
                "Metric '{}' is degrading (avg: {:.3}, trend: {:.3}). Consider adjusting parameters.",
                metric.name,
                metric.moving_average(),
                metric.trend()
            ));
        }
        suggestions
    }

    /// Résumé texte des performances.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        for metric in &self.metrics {
            parts.push(format!(
                "{}: avg={:.3} min={:.3} max={:.3} trend={:+.3}",
                metric.name,
                metric.moving_average(),
                metric.min(),
                metric.max(),
                metric.trend()
            ));
        }
        parts.join(" | ")
    }
}

impl Default for SelfImprovementEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Statut de santé global du système.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    Improving,
    Stable,
    Degrading,
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── PerformanceMetric ──────────────────────────────────────────────

    #[test]
    fn test_metric_moving_average() {
        let mut m = PerformanceMetric::new("test", 5);
        m.record(1.0);
        m.record(2.0);
        m.record(3.0);
        assert!((m.moving_average() - 2.0).abs() < 0.01);
    }

    #[test]
    fn test_metric_trend_improving() {
        let mut m = PerformanceMetric::new("test", 10);
        for v in [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8] {
            m.record(v);
        }
        assert!(m.trend() > 0.0);
        assert!(m.is_improving());
    }

    #[test]
    fn test_metric_trend_degrading() {
        let mut m = PerformanceMetric::new("test", 10);
        for v in [0.8, 0.7, 0.6, 0.5, 0.4, 0.3, 0.2, 0.1] {
            m.record(v);
        }
        assert!(m.trend() < 0.0);
        assert!(!m.is_improving());
    }

    #[test]
    fn test_metric_window_size() {
        let mut m = PerformanceMetric::new("test", 3);
        m.record(1.0);
        m.record(2.0);
        m.record(3.0);
        m.record(4.0); // 1.0 sort
        assert_eq!(m.values.len(), 3);
        assert!((m.moving_average() - 3.0).abs() < 0.01); // (2+3+4)/3
    }

    // ── ErrorRecovery ──────────────────────────────────────────────────

    #[test]
    fn test_error_recovery_transient_retry() {
        let recovery = ErrorRecovery::new(3);
        let ctx = ErrorContext {
            operation: String::from("test"),
            error_message: String::from("connection timeout"),
            attempt: 0,
            max_attempts: 3,
            is_fatal: false,
        };
        assert_eq!(recovery.decide(&ctx), RecoveryStrategy::Retry);
    }

    #[test]
    fn test_error_recovery_fatal_abort() {
        let recovery = ErrorRecovery::new(3);
        let ctx = ErrorContext {
            operation: String::from("test"),
            error_message: String::from("FATAL: memory corruption"),
            attempt: 0,
            max_attempts: 3,
            is_fatal: true,
        };
        assert_eq!(recovery.decide(&ctx), RecoveryStrategy::Abort);
    }

    #[test]
    fn test_error_recovery_max_retries_exceeded() {
        let recovery = ErrorRecovery::new(3);
        let ctx = ErrorContext {
            operation: String::from("test"),
            error_message: String::from("something failed"),
            attempt: 3,
            max_attempts: 3,
            is_fatal: false,
        };
        assert_eq!(recovery.decide(&ctx), RecoveryStrategy::Abort);
    }

    #[test]
    fn test_error_recovery_fallback() {
        let mut recovery = ErrorRecovery::new(2);
        recovery.add_fallback("NVRAM", "use_disk_fallback");
        let ctx = ErrorContext {
            operation: String::from("save"),
            error_message: String::from("NVRAM write failed"),
            attempt: 2,
            max_attempts: 2,
            is_fatal: false,
        };
        assert_eq!(recovery.decide(&ctx), RecoveryStrategy::Fallback);
        assert_eq!(
            recovery.get_fallback("NVRAM write failed"),
            Some("use_disk_fallback")
        );
    }

    #[test]
    fn test_retry_delay_backoff() {
        let recovery = ErrorRecovery::new(5);
        let d0 = recovery.retry_delay_ms(0);
        let d1 = recovery.retry_delay_ms(1);
        let d2 = recovery.retry_delay_ms(2);
        assert!(d1 > d0, "Delay should increase (backoff)");
        assert!(d2 > d1, "Delay should increase (backoff)");
    }

    // ── TreeOfThoughts ─────────────────────────────────────────────────

    #[test]
    fn test_tot_root_creation() {
        let mut tot = TreeOfThoughts::new(3, 5, 0.9);
        let root_id = tot.root("Start");
        assert_eq!(root_id, 0);
        assert_eq!(tot.node_count(), 1);
    }

    #[test]
    fn test_tot_search_finds_path() {
        let mut tot = TreeOfThoughts::new(3, 5, 0.9);
        tot.root("Find best serial format");

        let expand: &ExpandFn = &|node: &ThoughtNode| -> Vec<String> {
            if node.depth == 0 {
                vec![
                    String::from("Dell format (7 alphanum)"),
                    String::from("HP format (10 mixed)"),
                    String::from("Lenovo format (PF + 8 digits)"),
                ]
            } else {
                vec![]
            }
        };

        let evaluate: &EvaluateFn = &|content: &str| -> f32 {
            if content.contains("Dell") {
                0.9
            } else if content.contains("HP") {
                0.6
            } else {
                0.4
            }
        };

        let path = tot.search(expand, evaluate);
        assert!(!path.is_empty());
        assert_eq!(path[0].content, "Find best serial format");
        assert!(
            tot.best_score() >= 0.9 || tot.best_score() >= 0.6,
            "Best score should be at least 0.6"
        );
    }

    // ── SelfImprovementEngine ───────────────────────────────────────────

    #[test]
    fn test_self_improvement_health() {
        let mut engine = SelfImprovementEngine::new();
        engine.track("patch_success_rate", 5);

        // Scores qui s'améliorent
        for v in [0.5, 0.6, 0.7, 0.8, 0.9] {
            engine.record("patch_success_rate", v);
        }

        assert_eq!(engine.health_check(), HealthStatus::Improving);
    }

    #[test]
    fn test_self_improvement_summary() {
        let mut engine = SelfImprovementEngine::new();
        engine.track("quality", 3);
        engine.record("quality", 0.7);
        engine.record("quality", 0.8);

        let summary = engine.summary();
        assert!(summary.contains("quality"));
        assert!(summary.contains("avg"));
    }

    // ── ThoughtNode ─────────────────────────────────────────────────────

    #[test]
    fn test_thought_node_tree() {
        let root = ThoughtNode::new(0, "root", 0, None);
        assert!(root.is_leaf());
        assert_eq!(root.depth, 0);

        let child = ThoughtNode::new(1, "child", 1, Some(0));
        assert_eq!(child.parent_id, Some(0));
        assert_eq!(child.depth, 1);
    }
}
