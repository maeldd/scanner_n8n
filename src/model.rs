//! Modèle métier de la passerelle de scan : action, code scanné et mouvement
//! en attente (avec compteur de quantité).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// Quantité maximale acceptée pour un mouvement. Sert de garde-fou contre une
/// touche bloquée ou une saisie aberrante ; le serveur peut appliquer sa propre
/// limite.
pub const MAX_QTY: u32 = 9999;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Action {
    #[serde(rename = "IN")]
    In,
    #[serde(rename = "OUT")]
    Out,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::In => "IN",
            Self::Out => "OUT",
        }
    }

    pub fn label_fr(self) -> &'static str {
        match self {
            Self::In => "ENTRÉE",
            Self::Out => "SORTIE",
        }
    }

    pub fn opposite(self) -> Self {
        match self {
            Self::In => Self::Out,
            Self::Out => Self::In,
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code.trim().to_ascii_uppercase().as_str() {
            "IN" => Some(Self::In),
            "OUT" => Some(Self::Out),
            _ => None,
        }
    }
}

/// Forme normalisée d'un code scanné : `IN|REF-001` ou `OUT|REF-001`.
///
/// Le QR ne contient volontairement que l'action et l'identifiant de référence :
/// la quantité est saisie dans l'application, le nom et le stock viennent du
/// catalogue.
pub fn parse_scan(raw: &str) -> Result<(Action, String, String), String> {
    let cleaned = raw.trim_matches(|c: char| c.is_whitespace() || c.is_control());
    if cleaned.is_empty() {
        return Err("Code vide.".to_string());
    }

    let (prefix, rest) = cleaned
        .split_once('|')
        .ok_or_else(|| "Format attendu : IN|REF-001 ou OUT|REF-001.".to_string())?;

    let action = Action::from_code(prefix)
        .ok_or_else(|| format!("Action inconnue « {} » (attendu IN ou OUT).", prefix.trim()))?;

    let sku = rest.trim().to_ascii_uppercase();
    if sku.is_empty() {
        return Err("Référence vide après le séparateur « | ».".to_string());
    }
    if sku.len() > 64 {
        return Err("Référence trop longue (64 caractères maximum).".to_string());
    }
    if !sku
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(format!(
            "Référence invalide « {sku} » : lettres, chiffres, point, tiret et souligné uniquement."
        ));
    }

    Ok((action, sku.clone(), format!("{}|{sku}", action.as_str())))
}

#[derive(Clone, PartialEq, Debug)]
pub struct PendingMovement {
    pub sku: String,
    pub action: Action,
    pub qty: u32,
    pub scanned_at: DateTime<Utc>,
    pub raw: String,
    pub last_scan: Instant,
    pub last_input: Instant,
    /// Vrai dès que l'opérateur a saisi un chiffre : le chiffre suivant sera
    /// ajouté au lieu de remplacer la quantité.
    pub qty_edited: bool,
    /// Vrai après un refus serveur (stock insuffisant) : la validation
    /// automatique est suspendue tant que l'opérateur n'a pas ajusté la
    /// quantité.
    pub needs_review: bool,
}

impl PendingMovement {
    fn new(action: Action, sku: String, raw: String, now: Instant, wall: DateTime<Utc>) -> Self {
        Self {
            sku,
            action,
            qty: 1,
            scanned_at: wall,
            raw,
            last_scan: now,
            last_input: now,
            qty_edited: false,
            needs_review: false,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum ScanResult {
    /// Double lecture du même code dans la fenêtre anti-doublon.
    Ignored,
    /// Premier scan : le mouvement attend la validation.
    Started,
    /// Même code scanné volontairement : quantité +1.
    Incremented,
    /// Un autre code a été scanné : le mouvement précédent doit être validé.
    CommittedThenStarted(PendingMovement),
    /// Le mouvement précédent a été refusé (stock insuffisant) et attend une
    /// correction : les nouveaux scans sont bloqués pour ne pas le rejouer.
    NeedsReview,
}

/// Machine à états du mouvement en attente.
///
/// Le scan suivant valide le mouvement courant, ce qui permet de scanner une
/// série de références sans appuyer sur Entrée à chaque fois. Entrée valide
/// immédiatement, Échap annule, et un délai d'inactivité valide automatiquement.
pub struct MovementState {
    pending: Option<PendingMovement>,
    debounce: Duration,
    auto_commit: Duration,
}

impl MovementState {
    pub fn new(debounce: Duration, auto_commit: Duration) -> Self {
        Self {
            pending: None,
            debounce,
            auto_commit,
        }
    }

    pub fn pending(&self) -> Option<&PendingMovement> {
        self.pending.as_ref()
    }

    pub fn scan(
        &mut self,
        action: Action,
        sku: String,
        raw: String,
        now: Instant,
        wall: DateTime<Utc>,
    ) -> ScanResult {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.needs_review)
        {
            return ScanResult::NeedsReview;
        }

        if let Some(pending) = &mut self.pending {
            if pending.sku == sku && pending.action == action {
                if now.saturating_duration_since(pending.last_scan) < self.debounce {
                    return ScanResult::Ignored;
                }
                pending.qty = pending.qty.saturating_add(1).min(MAX_QTY);
                pending.last_scan = now;
                pending.last_input = now;
                // Après un scan, une saisie clavier doit remplacer le compteur.
                pending.qty_edited = false;
                return ScanResult::Incremented;
            }

            let previous = self.pending.take().expect("mouvement en attente");
            self.pending = Some(PendingMovement::new(action, sku, raw, now, wall));
            return ScanResult::CommittedThenStarted(previous);
        }

        self.pending = Some(PendingMovement::new(action, sku, raw, now, wall));
        ScanResult::Started
    }

    /// Saisie d'un chiffre au pavé numérique.
    pub fn digit(&mut self, digit: u32, now: Instant) {
        let Some(pending) = &mut self.pending else {
            return;
        };
        let digit = digit.min(9);
        if pending.qty_edited {
            pending.qty = pending
                .qty
                .saturating_mul(10)
                .saturating_add(digit)
                .min(MAX_QTY);
        } else {
            pending.qty = digit;
            pending.qty_edited = true;
        }
        pending.last_input = now;
        pending.needs_review = false;
    }

    pub fn backspace(&mut self, now: Instant) {
        let Some(pending) = &mut self.pending else {
            return;
        };
        if !pending.qty_edited {
            return;
        }
        pending.qty /= 10;
        if pending.qty == 0 {
            pending.qty = 1;
            pending.qty_edited = false;
        }
        pending.last_input = now;
        pending.needs_review = false;
    }

    pub fn adjust(&mut self, delta: i32, now: Instant) {
        let Some(pending) = &mut self.pending else {
            return;
        };
        if delta >= 0 {
            pending.qty = pending.qty.saturating_add(delta as u32).min(MAX_QTY);
        } else {
            pending.qty = pending.qty.saturating_sub(delta.unsigned_abs()).max(1);
        }
        pending.qty_edited = true;
        pending.last_input = now;
        pending.needs_review = false;
    }

    /// Retire le mouvement en attente pour le valider.
    pub fn take(&mut self) -> Option<PendingMovement> {
        self.pending.take()
    }

    /// Annule le mouvement en attente.
    pub fn cancel(&mut self) -> Option<PendingMovement> {
        self.pending.take()
    }

    /// Validation automatique après inactivité.
    pub fn tick(&mut self, now: Instant) -> Option<PendingMovement> {
        let should_commit = self.pending.as_ref().is_some_and(|p| {
            p.qty > 0
                && !p.needs_review
                && now.saturating_duration_since(p.last_input) >= self.auto_commit
        });
        if should_commit {
            self.pending.take()
        } else {
            None
        }
    }

    /// Réouvre un mouvement après un refus serveur (par exemple stock
    /// insuffisant), pour que l'opérateur puisse corriger la quantité sans
    /// rescanner.
    pub fn restore(
        &mut self,
        action: Action,
        sku: String,
        qty: u32,
        raw: String,
        wall: DateTime<Utc>,
    ) {
        let now = Instant::now();
        self.pending = Some(PendingMovement {
            sku,
            action,
            qty: qty.clamp(1, MAX_QTY),
            scanned_at: wall,
            raw,
            last_scan: now,
            last_input: now,
            qty_edited: false,
            needs_review: true,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn parse_scan_normalizes() {
        let (action, sku, raw) = parse_scan(" in | dco001 ").unwrap();
        assert_eq!(action, Action::In);
        assert_eq!(sku, "DCO001");
        assert_eq!(raw, "IN|DCO001");
    }

    #[test]
    fn parse_scan_rejects_legacy_barcode() {
        assert!(parse_scan("3760123456789").is_err());
        assert!(parse_scan("IN|").is_err());
        assert!(parse_scan("IN|VIS M3").is_err());
        assert!(parse_scan("MOVE|DCO001").is_err());
    }

    #[test]
    fn repeated_scan_increments_after_debounce() {
        let start = Instant::now();
        let wall = Utc::now();
        let mut state = MovementState::new(Duration::from_millis(300), Duration::from_secs(5));

        assert_eq!(
            state.scan(
                Action::Out,
                "DCO001".into(),
                "OUT|DCO001".into(),
                start,
                wall
            ),
            ScanResult::Started
        );
        assert_eq!(state.pending().unwrap().qty, 1);

        // Double lecture immédiate : ignorée.
        assert_eq!(
            state.scan(
                Action::Out,
                "DCO001".into(),
                "OUT|DCO001".into(),
                start + Duration::from_millis(100),
                wall
            ),
            ScanResult::Ignored
        );
        assert_eq!(state.pending().unwrap().qty, 1);

        // Scan volontaire : +1.
        assert_eq!(
            state.scan(
                Action::Out,
                "DCO001".into(),
                "OUT|DCO001".into(),
                start + Duration::from_millis(500),
                wall
            ),
            ScanResult::Incremented
        );
        assert_eq!(state.pending().unwrap().qty, 2);
    }

    #[test]
    fn scanning_another_code_commits_previous() {
        let start = Instant::now();
        let wall = Utc::now();
        let mut state = MovementState::new(Duration::from_millis(300), Duration::from_secs(5));

        state.scan(
            Action::Out,
            "DCO001".into(),
            "OUT|DCO001".into(),
            start,
            wall,
        );
        let result = state.scan(
            Action::In,
            "DA001".into(),
            "IN|DA001".into(),
            start + Duration::from_millis(800),
            wall,
        );

        match result {
            ScanResult::CommittedThenStarted(previous) => {
                assert_eq!(previous.sku, "DCO001");
                assert_eq!(previous.qty, 1);
            }
            other => panic!("résultat inattendu : {other:?}"),
        }
        assert_eq!(state.pending().unwrap().sku, "DA001");
    }

    #[test]
    fn keyboard_replaces_then_appends() {
        let now = Instant::now();
        let wall = Utc::now();
        let mut state = MovementState::new(Duration::from_millis(300), Duration::from_secs(5));
        state.scan(Action::Out, "DCO001".into(), "OUT|DCO001".into(), now, wall);

        state.digit(1, now);
        state.digit(0, now);
        state.digit(0, now);
        assert_eq!(state.pending().unwrap().qty, 100);

        state.backspace(now);
        assert_eq!(state.pending().unwrap().qty, 10);
    }

    #[test]
    fn tick_commits_after_idle() {
        let start = Instant::now();
        let wall = Utc::now();
        let mut state = MovementState::new(Duration::from_millis(300), Duration::from_secs(5));
        state.scan(Action::In, "DCO001".into(), "IN|DCO001".into(), start, wall);

        assert!(state.tick(start + Duration::from_secs(4)).is_none());
        let committed = state.tick(start + Duration::from_secs(6)).unwrap();
        assert_eq!(committed.qty, 1);
        assert!(state.pending().is_none());
    }

    #[test]
    fn rejected_movement_is_not_auto_committed_or_replaced_by_scan() {
        let mut state = MovementState::new(Duration::from_millis(100), Duration::from_secs(1));
        state.restore(
            Action::Out,
            "DCO001".to_string(),
            4,
            "OUT|DCO001".to_string(),
            Utc::now(),
        );

        let start = Instant::now();
        assert!(state.tick(start + Duration::from_secs(30)).is_none());

        let result = state.scan(
            Action::In,
            "DA001".to_string(),
            "IN|DA001".to_string(),
            start,
            Utc::now(),
        );
        assert!(matches!(result, ScanResult::NeedsReview));
        assert_eq!(state.pending().unwrap().sku, "DCO001");

        // Un ajustement clavier débloque la validation automatique.
        state.digit(2, start + Duration::from_secs(31));
        assert_eq!(state.pending().unwrap().qty, 2);
        assert!(state.tick(start + Duration::from_secs(33)).is_some());
    }
}
