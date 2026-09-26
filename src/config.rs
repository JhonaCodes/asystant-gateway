use std::{collections::HashSet, env};
use serde::{Deserialize, Serialize};
use crate::{
    error::AppError,
    origins::{self, MAX_ORIGINS},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetOverride {
    pub tenant: String,
    pub subject: Option<String>,
    pub daily_micros: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientModels {
    pub tenant: String,
    #[serde(default)]
    pub subject: Option<String>,
    pub models: Vec<String>,
    pub default_model: String,
    #[serde(default)]
    pub allow_selection: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPolicy {
    pub models: Vec<String>,
    pub default_model: String,
    pub allow_selection: bool,
}
impl ModelPolicy {
    pub fn permits(&self, model: &str) -> bool {
        self.models.iter().any(|m| m == model)
            && (self.allow_selection || model == self.default_model)
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Product {
    pub issuer: String,
    pub secret: String,
    pub daily_tenant_micros: i64,
    pub daily_user_micros: i64,
    pub models: Vec<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub client_models: Vec<ClientModels>,
    #[serde(default)]
    pub budget_overrides: Vec<BudgetOverride>,
}
impl Product {
    pub fn model_policy(&self, tenant: &str, subject: &str) -> Result<ModelPolicy, AppError> {
        let client = self
            .client_models
            .iter()
            .find(|c| c.tenant == tenant && c.subject.as_deref() == Some(subject))
            .or_else(|| {
                self.client_models
                    .iter()
                    .find(|c| c.tenant == tenant && c.subject.is_none())
            });
        let (models, default_model, allow_selection) = match client {
            Some(c) => (c.models.clone(), c.default_model.clone(), c.allow_selection),
            None => (
                self.models.clone(),
                self.default_model
                    .clone()
                    .or_else(|| self.models.first().cloned())
                    .ok_or(AppError::Invalid)?,
                true,
            ),
        };
        if !models.contains(&default_model) || models.iter().any(|m| !self.models.contains(m)) {
            return Err(AppError::Invalid);
        }
        Ok(ModelPolicy {
            models: if allow_selection {
                models
            } else {
                vec![default_model.clone()]
            },
            default_model,
            allow_selection,
        })
    }

    pub fn limits(&self, tenant: &str, subject: &str) -> (i64, i64) {
        let tenant_limit = self
            .budget_overrides
            .iter()
            .find(|b| b.tenant == tenant && b.subject.is_none())
            .map_or(self.daily_tenant_micros, |b| b.daily_micros);
        let user_limit = self
            .budget_overrides
            .iter()
            .find(|b| b.tenant == tenant && b.subject.as_deref() == Some(subject))
            .map_or(self.daily_user_micros, |b| b.daily_micros);
        (tenant_limit, user_limit)
    }
}
impl std::fmt::Debug for Product {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Product")
            .field("issuer", &self.issuer)
            .finish_non_exhaustive()
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Openrouter,
    Openai,
    Gemini,
    Anthropic,
    OpencodeZen,
    OpencodeGo,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WireApi {
    #[default]
    ChatCompletions,
    Messages,
    Responses,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub id: String,
    pub provider: Provider,
    pub model: String,
    pub key_env: String,
    #[serde(default)]
    pub wire_api: WireApi,
    pub input_micros_per_million: i64,
    pub output_micros_per_million: i64,
    pub max_input_tokens: u32,
    pub max_output_tokens: u32,
}
impl ModelConfig {
    pub fn reservation(&self) -> Result<i64, AppError> {
        let input = i64::from(self.max_input_tokens)
            .checked_mul(self.input_micros_per_million)
            .ok_or(AppError::Invalid)?;
        let output = i64::from(self.max_output_tokens)
            .checked_mul(self.output_micros_per_million)
            .ok_or(AppError::Invalid)?;
        input
            .checked_add(output)
            .and_then(|n| n.checked_add(999_999))
            .map(|n| n / 1_000_000)
            .ok_or(AppError::Invalid)
    }
    pub fn validate(&self) -> Result<(), AppError> {
        if self.id.is_empty()
            || self.model.is_empty()
            || self.input_micros_per_million <= 0
            || self.output_micros_per_million <= 0
            || self.max_input_tokens == 0
            || self.max_output_tokens == 0
            || self.max_input_tokens > 1_000_000
            || self.max_output_tokens > 32768
        {
            return Err(AppError::Invalid);
        }
        match self.wire_api {
            WireApi::Messages
                if !matches!(
                    self.provider,
                    Provider::Anthropic | Provider::OpencodeZen | Provider::OpencodeGo
                ) =>
            {
                return Err(AppError::Invalid);
            }
            WireApi::Responses
                if !matches!(
                    self.provider,
                    Provider::Openai | Provider::OpencodeZen | Provider::OpencodeGo
                ) =>
            {
                return Err(AppError::Invalid);
            }
            _ => {}
        }
        self.reservation()?;
        Ok(())
    }
}
#[derive(Debug, Clone)]
pub struct Config {
    pub products: Vec<Product>,
    pub models: Vec<ModelConfig>,
    pub origins: Vec<String>,
}
impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let config = Self {
            products: serde_json::from_str(&env::var("ASYSTANT_PRODUCTS")?)?,
            models: serde_json::from_str(&env::var("ASYSTANT_MODELS")?)?,
            // Only the initial list: once administration saves the policy, the
            // origins managed there replace this value.
            origins: env::var("ASYSTANT_ORIGINS")
                .unwrap_or_default()
                .split(',')
                .filter(|s| !s.trim().is_empty())
                .map(origins::normalize)
                .collect::<Result<_, _>>()?,
        };
        config.validate()?;
        for model in &config.models {
            if env::var(&model.key_env)?.is_empty() {
                anyhow::bail!("empty provider credential")
            }
        }
        Ok(config)
    }
    pub fn validate(&self) -> Result<(), AppError> {
        if self.products.is_empty() || self.models.is_empty() {
            return Err(AppError::Invalid);
        }
        let mut ids = HashSet::new();
        for model in &self.models {
            model.validate()?;
            if !ids.insert(&model.id) {
                return Err(AppError::Invalid);
            }
        }
        if self.origins.len() > MAX_ORIGINS
            || self.origins.iter().collect::<HashSet<_>>().len() != self.origins.len()
            || self
                .origins
                .iter()
                .any(|o| origins::normalize(o).ok().as_deref() != Some(o.as_str()))
        {
            return Err(AppError::Invalid);
        }
        let mut issuers = HashSet::new();
        for p in &self.products {
            if p.default_model
                .as_ref()
                .is_some_and(|m| !p.models.contains(m))
            {
                return Err(AppError::Invalid);
            }
            let mut clients = HashSet::new();
            for c in &p.client_models {
                if c.tenant.is_empty()
                    || c.subject.as_ref().is_some_and(|s| s.is_empty())
                    || c.models.is_empty()
                    || !c.models.contains(&c.default_model)
                    || c.models.iter().any(|m| !p.models.contains(m))
                    || c.models.iter().collect::<HashSet<_>>().len() != c.models.len()
                    || !clients.insert((&c.tenant, &c.subject))
                {
                    return Err(AppError::Invalid);
                }
            }
            let mut budgets = HashSet::new();
            for budget in &p.budget_overrides {
                if budget.tenant.is_empty()
                    || budget.daily_micros < 0
                    || !budgets.insert((&budget.tenant, &budget.subject))
                {
                    return Err(AppError::Invalid);
                }
            }

            if p.secret.len() < 32
                || p.issuer.is_empty()
                || !issuers.insert(&p.issuer)
                || p.daily_tenant_micros <= 0
                || p.daily_user_micros <= 0
                || p.models.is_empty()
                || p.models.iter().any(|m| !ids.contains(m))
            {
                return Err(AppError::Invalid);
            }
        }
        Ok(())
    }
}
