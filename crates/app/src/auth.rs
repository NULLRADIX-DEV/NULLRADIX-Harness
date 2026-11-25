//! Credentials of CLI clients and browser sessions.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nullradix_domain::{Id, Timestamp};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};

use crate::{AppError, Clock, IdGenerator};

const SECRET_BYTES: usize = 32;
const HANDOFF_LIFETIME: Duration = Duration::from_secs(60);
const SESSION_LIFETIME: Duration = Duration::from_hours(30 * 24);

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
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
}

/// Who sent a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Principal {
    Token { id: Id<ApiToken>, kind: TokenKind },
    Session { id: Id<Session> },
}

#[async_trait]
pub trait AuthRepository: Send + Sync {
    async fn token_by_hash(&self, hash: &SecretHash) -> Result<Option<ApiToken>, AppError>;
    /// Revokes every token with the same kind and name, then stores `token`.
    async fn replace_tokens(&self, token: &ApiToken) -> Result<(), AppError>;
    async fn session_by_hash(&self, hash: &SecretHash) -> Result<Option<Session>, AppError>;
    async fn insert_session(&self, session: &Session) -> Result<(), AppError>;
    async fn delete_session(&self, id: Id<Session>) -> Result<(), AppError>;
    async fn delete_expired_sessions(&self, now: Timestamp) -> Result<u64, AppError>;
}

pub struct AuthService {
    repository: Arc<dyn AuthRepository>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    handoffs: Mutex<HashMap<SecretHash, Timestamp>>,
}

impl fmt::Debug for AuthService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthService").finish_non_exhaustive()
    }
}

fn unauthorized(code: &'static str, message: &str) -> AppError {
    AppError::unauthorized(code, message)
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

    /// One-time code a token holder exchanges for a browser session within a minute.
    pub fn create_handoff(&self) -> Result<Secret, AppError> {
        let now = self.clock.now();
        let code = Secret::generate()?;
        let mut handoffs = self.handoffs.lock();
        handoffs.retain(|_, expires_at| *expires_at > now);
        handoffs.insert(code.hash(), later(now, HANDOFF_LIFETIME)?);
        Ok(code)
    }

    /// Redeems a handoff code and returns the secret of the new session.
    pub async fn redeem_handoff(&self, code: &Secret) -> Result<Secret, AppError> {
        let now = self.clock.now();
        let expires_at = self.handoffs.lock().remove(&code.hash());
        if expires_at.is_none_or(|expires_at| expires_at <= now) {
            return Err(unauthorized(
                "handoff-invalid",
                "the handoff code is unknown or expired",
            ));
        }
        let secret = Secret::generate()?;
        let session = Session {
            id: self.ids.as_ref().next(),
            hash: secret.hash(),
            created_at: now,
            expires_at: later(now, SESSION_LIFETIME)?,
        };
        self.repository.insert_session(&session).await?;
        Ok(secret)
    }

    pub async fn authenticate_session(&self, secret: &Secret) -> Result<Principal, AppError> {
        let session = self
            .repository
            .session_by_hash(&secret.hash())
            .await?
            .ok_or_else(|| unauthorized("session-invalid", "the session is not valid"))?;
        if session.expires_at <= self.clock.now() {
            self.repository.delete_session(session.id).await?;
            return Err(unauthorized("session-expired", "the session has expired"));
        }
        Ok(Principal::Session { id: session.id })
    }

    /// Removes sessions that can no longer be used; returns how many were removed.
    pub async fn prune_expired_sessions(&self) -> Result<u64, AppError> {
        self.repository
            .delete_expired_sessions(self.clock.now())
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
