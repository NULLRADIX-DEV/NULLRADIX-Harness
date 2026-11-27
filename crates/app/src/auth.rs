//! Credentials of CLI clients and browser sessions.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nullradix_domain::{Id, Timestamp};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};

use crate::{AppError, Change, Clock, FieldProblem, IdGenerator};

/// Entity id of the step-up switch in the change feed, which only takes UUIDs.
pub const STEP_UP_FEED_ID: uuid::Uuid =
    uuid::Uuid::from_u128(0x6e75_6c6c_7261_6469_7800_7374_6570_7570);

const SECRET_BYTES: usize = 32;
/// A handoff code is opened at once by whoever asked for it.
const HANDOFF_LIFETIME: Duration = Duration::from_secs(15);
/// A session ends this long after it began, whatever it did since.
const SESSION_LIFETIME: Duration = Duration::from_hours(7 * 24);
/// A session ends this long after its last request.
const SESSION_IDLE: Duration = Duration::from_hours(12);
/// The time of the last request is only written back this often.
const SEEN_EVERY: Duration = Duration::from_mins(1);
/// A step-up code is passed on by the person after a terminal printed it.
const STEP_UP_CODE_LIFETIME: Duration = Duration::from_mins(2);
/// How long a confirmed session may change the command fields.
const ELEVATION: Duration = Duration::from_mins(10);
/// Codes that wait to be redeemed; the oldest make room.
const PENDING_CODES: usize = 64;

/// Random credential shown to a client once; only its hash is stored.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    fn generate() -> Result<Self, AppError> {
        let mut bytes = [0_u8; SECRET_BYTES];
        getrandom::fill(&mut bytes).map_err(|error| {
            AppError::internal(std::io::Error::other(format!(
                "no system randomness: {error}"
            )))
        })?;
        Ok(Self(hex(&bytes)))
    }

    /// Accepts only well-formed secrets so garbage never reaches the database.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let valid = text.len() == SECRET_BYTES * 2
            && text
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
        valid.then(|| Self(text.to_owned()))
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn hash(&self) -> SecretHash {
        SecretHash(hex(&Sha256::digest(self.0.as_bytes())))
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(..)")
    }
}

/// SHA-256 of a secret, hex encoded.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SecretHash(String);

impl SecretHash {
    #[must_use]
    pub fn from_stored(hex: String) -> Self {
        Self(hex)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    text
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Cli,
}

impl TokenKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        (text == "cli").then_some(Self::Cli)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiToken {
    pub id: Id<ApiToken>,
    pub name: String,
    pub kind: TokenKind,
    pub hash: SecretHash,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: Id<Session>,
    pub hash: SecretHash,
    /// Hash of the key only the tab that opened the session holds.
    pub key_hash: SecretHash,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    pub last_seen_at: Timestamp,
    /// Until when the session was confirmed to change command fields.
    pub elevated_until: Option<Timestamp>,
}

/// The secrets of a new session: the cookie value and the key the tab keeps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSecrets {
    pub session: Secret,
    pub key: Secret,
}

/// A run or session of an agent tried to decide something that stays with the person.
#[must_use]
pub fn decision_needs_person(action: &str) -> AppError {
    AppError::forbidden(
        "decision-needs-person",
        format!(
            "{action} stays with the person: do it in the app or in a terminal outside the run"
        ),
    )
}

/// Who sent a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Principal {
    Token {
        id: Id<ApiToken>,
        kind: TokenKind,
    },
    Session {
        id: Id<Session>,
        /// Until when the session is confirmed to change command fields, if it still is.
        elevated_until: Option<Timestamp>,
    },
}

/// Who asks to change fields that end up on a command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandWriter {
    /// The person in the app, with or without a fresh confirmation.
    Person { elevated: bool },
    /// A token, or anything that names an agent.
    Other,
}

/// Why command fields may not change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandRefusal {
    NeedsPerson,
    NeedsStepUp,
}

/// How the person gets a confirmation, as the refusal tells it.
const HOW_TO_CONFIRM: &str = "run `nullradix confirm` in a terminal and open the link";
/// The same through the tray menu of the desktop shell.
const HOW_TO_CONFIRM_TRAY: &str = "or choose Confirm changes in the tray menu";

/// The code is unknown, expired or used up.
#[must_use]
pub fn step_up_code_invalid() -> AppError {
    AppError::invalid(
        "step-up-code-invalid",
        "the confirmation code is unknown or expired",
    )
    .with_fields(vec![FieldProblem {
        field: String::from("code"),
        message: String::from("run nullradix confirm again for a new code"),
    }])
}

impl CommandRefusal {
    /// The error, naming each field that was about to change.
    #[must_use]
    pub fn error(self, fields: &[&str]) -> AppError {
        let message = match self {
            Self::NeedsPerson => "only the person in the app can change this",
            Self::NeedsStepUp => "needs a confirmation first",
        };
        let problems = fields
            .iter()
            .map(|field| FieldProblem {
                field: (*field).to_owned(),
                message: message.to_owned(),
            })
            .collect();
        match self {
            Self::NeedsPerson => AppError::forbidden(
                "command-change-needs-person",
                "fields that run commands can only be changed by the person in the app",
            ),
            Self::NeedsStepUp => AppError::forbidden(
                "step-up-required",
                format!(
                    "changing fields that run commands needs a confirmation: {HOW_TO_CONFIRM}, {HOW_TO_CONFIRM_TRAY}, then save again"
                ),
            ),
        }
        .with_fields(problems)
    }
}

/// Turning the confirmation off needs a code of its own, not only a confirmed session.
#[must_use]
pub fn step_up_code_required() -> AppError {
    AppError::forbidden(
        "step-up-code-required",
        "turning the confirmation off needs a fresh code: run `nullradix confirm --no-open` and enter it",
    )
    .with_fields(vec![FieldProblem {
        field: String::from("code"),
        message: String::from("enter the code that nullradix confirm --no-open printed"),
    }])
}

#[async_trait]
pub trait AuthRepository: Send + Sync {
    async fn token_by_hash(&self, hash: &SecretHash) -> Result<Option<ApiToken>, AppError>;
    /// Revokes every token with the same kind and name, then stores `token`.
    async fn replace_tokens(&self, token: &ApiToken) -> Result<(), AppError>;
    async fn session_by_hash(&self, hash: &SecretHash) -> Result<Option<Session>, AppError>;
    async fn insert_session(&self, session: &Session) -> Result<(), AppError>;
    async fn delete_session(&self, id: Id<Session>) -> Result<(), AppError>;
    /// Removes sessions past their end or idle since `idle_before`.
    async fn delete_expired_sessions(
        &self,
        now: Timestamp,
        idle_before: Timestamp,
    ) -> Result<u64, AppError>;
    async fn touch_session(&self, id: Id<Session>, at: Timestamp) -> Result<(), AppError>;
    /// Confirms the session until `until`; `false` when it is gone.
    async fn elevate_session(&self, id: Id<Session>, until: Timestamp) -> Result<bool, AppError>;
    /// Whether command fields need a confirmation; on unless the person turned it off.
    async fn step_up_required(&self) -> Result<bool, AppError>;
    /// Stores the switch; `None` when it already had that value.
    async fn set_step_up_required(
        &self,
        required: bool,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError>;
}

/// What a request brought to prove its session.
#[derive(Clone, Copy)]
enum Presented<'a> {
    /// The key of the tab, if it sent one.
    Key(Option<&'a Secret>),
    /// Only the cookie, for a file a page loads.
    Asset,
}

pub struct AuthService {
    repository: Arc<dyn AuthRepository>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    handoffs: Mutex<HashMap<SecretHash, Timestamp>>,
    step_up_codes: Mutex<HashMap<SecretHash, Timestamp>>,
}

impl fmt::Debug for AuthService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthService").finish_non_exhaustive()
    }
}

fn unauthorized(code: &'static str, message: &str) -> AppError {
    AppError::unauthorized(code, message)
}

fn millis(span: Duration) -> i64 {
    i64::try_from(span.as_millis()).unwrap_or(i64::MAX)
}

fn later(now: Timestamp, lifetime: Duration) -> Result<Timestamp, AppError> {
    now.checked_add(lifetime)
        .ok_or_else(|| AppError::internal(std::io::Error::other("clock out of range")))
}

impl AuthService {
    #[must_use]
    pub fn new(
        repository: Arc<dyn AuthRepository>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
    ) -> Self {
        Self {
            repository,
            clock,
            ids,
            handoffs: Mutex::new(HashMap::new()),
            step_up_codes: Mutex::new(HashMap::new()),
        }
    }

    /// Keeps a still-registered local token or replaces all local tokens with a new one.
    pub async fn local_token(&self, existing: Option<Secret>) -> Result<Secret, AppError> {
        if let Some(secret) = existing
            && self
                .repository
                .token_by_hash(&secret.hash())
                .await?
                .is_some()
        {
            return Ok(secret);
        }
        let secret = Secret::generate()?;
        let token = ApiToken {
            id: self.ids.as_ref().next(),
            name: String::from("local"),
            kind: TokenKind::Cli,
            hash: secret.hash(),
            created_at: self.clock.now(),
        };
        self.repository.replace_tokens(&token).await?;
        Ok(secret)
    }

    pub async fn authenticate_token(&self, secret: &Secret) -> Result<Principal, AppError> {
        let token = self
            .repository
            .token_by_hash(&secret.hash())
            .await?
            .ok_or_else(|| unauthorized("token-invalid", "the token is not valid"))?;
        Ok(Principal::Token {
            id: token.id,
            kind: token.kind,
        })
    }

    /// One-time code a token holder exchanges for a browser session within seconds.
    pub fn create_handoff(&self) -> Result<Secret, AppError> {
        self.issue_code(&self.handoffs, HANDOFF_LIFETIME)
    }

    fn issue_code(
        &self,
        codes: &Mutex<HashMap<SecretHash, Timestamp>>,
        lifetime: Duration,
    ) -> Result<Secret, AppError> {
        let now = self.clock.now();
        let code = Secret::generate()?;
        let until = later(now, lifetime)?;
        let mut codes = codes.lock();
        codes.retain(|_, expires_at| *expires_at > now);
        if codes.len() >= PENDING_CODES
            && let Some(oldest) = codes
                .iter()
                .min_by_key(|(_, expires_at)| **expires_at)
                .map(|(hash, _)| hash.clone())
        {
            codes.remove(&oldest);
        }
        codes.insert(code.hash(), until);
        Ok(code)
    }

    /// Takes the code if it is known and alive; a code works once.
    fn spend_code(&self, codes: &Mutex<HashMap<SecretHash, Timestamp>>, code: &Secret) -> bool {
        let now = self.clock.now();
        let expires_at = codes.lock().remove(&code.hash());
        expires_at.is_some_and(|expires_at| expires_at > now)
    }

    /// Redeems a handoff code and returns the secrets of the new session.
    pub async fn redeem_handoff(&self, code: &Secret) -> Result<SessionSecrets, AppError> {
        if !self.spend_code(&self.handoffs, code) {
            return Err(unauthorized(
                "handoff-invalid",
                "the handoff code is unknown or expired",
            ));
        }
        let now = self.clock.now();
        let secrets = SessionSecrets {
            session: Secret::generate()?,
            key: Secret::generate()?,
        };
        let session = Session {
            id: self.ids.as_ref().next(),
            hash: secrets.session.hash(),
            key_hash: secrets.key.hash(),
            created_at: now,
            expires_at: later(now, SESSION_LIFETIME)?,
            last_seen_at: now,
            elevated_until: None,
        };
        self.repository.insert_session(&session).await?;
        Ok(secrets)
    }

    /// A session needs its cookie and the key of its tab; the cookie alone is nobody.
    pub async fn authenticate_session(
        &self,
        secret: &Secret,
        key: Option<&Secret>,
    ) -> Result<Principal, AppError> {
        self.session_principal(secret, Presented::Key(key)).await
    }

    /// For files a page loads without a key: a live cookie is enough, and it keeps nothing alive.
    pub async fn authenticate_asset_session(&self, secret: &Secret) -> Result<Principal, AppError> {
        self.session_principal(secret, Presented::Asset).await
    }

    async fn session_principal(
        &self,
        secret: &Secret,
        presented: Presented<'_>,
    ) -> Result<Principal, AppError> {
        let session = self
            .repository
            .session_by_hash(&secret.hash())
            .await?
            .ok_or_else(|| unauthorized("session-invalid", "the session is not valid"))?;
        let keyed = matches!(presented, Presented::Key(_));
        if let Presented::Key(brought) = presented {
            let brought = brought.ok_or_else(|| {
                unauthorized(
                    "session-key-missing",
                    "this tab has no key for the session: open NULLRADIX again",
                )
            })?;
            if brought.hash() != session.key_hash {
                return Err(unauthorized(
                    "session-key-invalid",
                    "the key does not belong to this session: open NULLRADIX again",
                ));
            }
        }
        let now = self.clock.now();
        let since_seen = now.as_millis() - session.last_seen_at.as_millis();
        if session.expires_at <= now || since_seen >= millis(SESSION_IDLE) {
            self.repository.delete_session(session.id).await?;
            return Err(unauthorized("session-expired", "the session has expired"));
        }
        if keyed && since_seen >= millis(SEEN_EVERY) {
            self.repository.touch_session(session.id, now).await?;
        }
        Ok(Principal::Session {
            id: session.id,
            // an asset request is no write, so it is never the confirmed one
            elevated_until: session.elevated_until.filter(|until| keyed && *until > now),
        })
    }

    /// A code a token holder hands to the person, who gives it to the page.
    pub fn create_step_up_code(&self) -> Result<Secret, AppError> {
        self.issue_code(&self.step_up_codes, STEP_UP_CODE_LIFETIME)
    }

    /// Uses up a code without confirming anyone; the person's proof for the one change that asks.
    pub fn spend_step_up_code(&self, code: &Secret) -> Result<(), AppError> {
        if self.spend_code(&self.step_up_codes, code) {
            Ok(())
        } else {
            Err(step_up_code_invalid())
        }
    }

    /// Confirms the session for a while; returns when that ends.
    pub async fn redeem_step_up(
        &self,
        session: Id<Session>,
        code: &Secret,
    ) -> Result<Timestamp, AppError> {
        self.spend_step_up_code(code)?;
        let until = later(self.clock.now(), ELEVATION)?;
        if !self.repository.elevate_session(session, until).await? {
            return Err(unauthorized("session-invalid", "the session is not valid"));
        }
        Ok(until)
    }

    /// Whether command fields need a confirmation; on until the person turns it off.
    pub async fn step_up_required(&self) -> Result<bool, AppError> {
        self.repository.step_up_required().await
    }

    /// Stores the switch; the change is the caller's to signal.
    pub async fn set_step_up_required(&self, required: bool) -> Result<Option<Change>, AppError> {
        self.repository
            .set_step_up_required(required, self.clock.now())
            .await
    }

    /// Why `writer` may not change command fields now; `None` means it may.
    pub async fn command_refusal(
        &self,
        writer: CommandWriter,
    ) -> Result<Option<CommandRefusal>, AppError> {
        Ok(match writer {
            CommandWriter::Other => Some(CommandRefusal::NeedsPerson),
            CommandWriter::Person { elevated } => {
                (!elevated && self.step_up_required().await?).then_some(CommandRefusal::NeedsStepUp)
            }
        })
    }

    /// Removes sessions that can no longer be used; returns how many were removed.
    pub async fn prune_expired_sessions(&self) -> Result<u64, AppError> {
        let now = self.clock.now();
        let idle_before = Timestamp::from_millis(now.as_millis() - millis(SESSION_IDLE));
        self.repository
            .delete_expired_sessions(now, idle_before)
            .await
    }

    pub async fn end_session(&self, secret: &Secret) -> Result<(), AppError> {
        if let Some(session) = self.repository.session_by_hash(&secret.hash()).await? {
            self.repository.delete_session(session.id).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_secrets_are_unique_and_parse_back() {
        let first = Secret::generate().unwrap();
        let second = Secret::generate().unwrap();
        assert_ne!(first, second);
        assert_eq!(Secret::parse(first.expose()), Some(first));
    }

    #[test]
    fn rejects_malformed_secrets() {
        assert_eq!(Secret::parse("abc"), None);
        assert_eq!(Secret::parse(&"G".repeat(64)), None);
        assert_eq!(Secret::parse(&"A".repeat(64)), None);
    }

    #[test]
    fn hashes_with_sha256_hex() {
        let secret = Secret::parse(&"a".repeat(64)).unwrap();
        assert_eq!(secret.hash().as_str().len(), 64);
        assert_eq!(secret.hash(), secret.hash());
        assert_eq!(hex(&[0x00, 0xab, 0xff]), "00abff");
    }

    #[test]
    fn never_prints_the_secret() {
        let secret = Secret::parse(&"a".repeat(64)).unwrap();
        assert_eq!(format!("{secret:?}"), "Secret(..)");
    }

    #[test]
    fn token_kinds_round_trip() {
        assert_eq!(
            TokenKind::parse(TokenKind::Cli.as_str()),
            Some(TokenKind::Cli)
        );
        assert_eq!(TokenKind::parse("root"), None);
    }
}
