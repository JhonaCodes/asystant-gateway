use serde::{Deserialize, Serialize};
use crate::{
    config::{BudgetOverride, ClientModels, Config},
    error::AppError,
    origins::{self, MAX_ORIGINS},
};

/// Only nonsecret operational settings are persisted or rendered in administration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Policy {
    pub products: Vec<ProductPolicy>,
    pub models: Vec<ModelLimits>,
    /// Browser origins allowed to call the API. Absent in policies saved before
    /// origins were managed here; the environment's initial list applies then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origins: Option<Vec<String>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductPolicy {
    pub issuer: String,
    pub models: Vec<String>,
    pub default_model: String,
    pub daily_tenant_micros: i64,
    pub daily_user_micros: i64,
    pub client_models: Vec<ClientModels>,
    pub budget_overrides: Vec<BudgetOverride>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelLimits {
    pub id: String,
    pub max_input_tokens: u32,
    pub max_output_tokens: u32,
}
impl Policy {
    pub fn from_config(config: &Config) -> Self {
        Self {
            products: config
                .products
                .iter()
                .map(|p| ProductPolicy {
                    issuer: p.issuer.clone(),
                    models: p.models.clone(),
                    default_model: p
                        .default_model
                        .clone()
                        .or_else(|| p.models.first().cloned())
                        .unwrap_or_default(),
                    daily_tenant_micros: p.daily_tenant_micros,
                    daily_user_micros: p.daily_user_micros,
                    client_models: p.client_models.clone(),
                    budget_overrides: p.budget_overrides.clone(),
                })
                .collect(),
            models: config
                .models
                .iter()
                .map(|m| ModelLimits {
                    id: m.id.clone(),
                    max_input_tokens: m.max_input_tokens,
                    max_output_tokens: m.max_output_tokens,
                })
                .collect(),
            origins: Some(config.origins.clone()),
        }
    }
    pub fn apply(&self, config: &mut Config) -> Result<(), AppError> {
        if let Some(origins) = &self.origins {
            config.origins = origins.clone();
        }
        for policy in &self.products {
            if let Some(p) = config
                .products
                .iter_mut()
                .find(|p| p.issuer == policy.issuer)
            {
                p.models = policy.models.clone();
                p.default_model = Some(policy.default_model.clone());
                p.daily_tenant_micros = policy.daily_tenant_micros;
                p.daily_user_micros = policy.daily_user_micros;
                p.client_models = policy.client_models.clone();
                p.budget_overrides = policy.budget_overrides.clone();
            }
        }
        for policy in &self.models {
            if let Some(m) = config.models.iter_mut().find(|m| m.id == policy.id) {
                m.max_input_tokens = policy.max_input_tokens;
                m.max_output_tokens = policy.max_output_tokens;
            }
        }
        config.validate()
    }
}
#[derive(Debug, Clone, Deserialize)]
pub struct EditPolicy {
    pub csrf: String,
    pub kind: String,
    #[serde(default)]
    pub issuer: String,
    #[serde(default)]
    pub tenant: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub daily_tenant_micros: i64,
    #[serde(default)]
    pub daily_user_micros: i64,
    #[serde(default)]
    pub max_input_tokens: u32,
    #[serde(default)]
    pub max_output_tokens: u32,
    #[serde(default)]
    pub origin: String,
}
impl EditPolicy {
    pub fn apply(&self, config: &mut Config) -> Result<(), AppError> {
        if self.kind == "origin_add" {
            let origin = origins::normalize(&self.origin)?;
            if !config.origins.contains(&origin) {
                if config.origins.len() >= MAX_ORIGINS {
                    return Err(AppError::Invalid);
                }
                config.origins.push(origin);
            }
        } else if self.kind == "origin_remove" {
            let origin = origins::normalize(&self.origin)?;
            config.origins.retain(|o| *o != origin);
        } else if self.kind == "model" {
            let m = config
                .models
                .iter_mut()
                .find(|m| m.id == self.model)
                .ok_or(AppError::Invalid)?;
            m.max_input_tokens = self.max_input_tokens;
            m.max_output_tokens = self.max_output_tokens;
        } else {
            let p = config
                .products
                .iter_mut()
                .find(|p| p.issuer == self.issuer)
                .ok_or(AppError::Invalid)?;
            match self.kind.as_str() {
                "product" => {
                    if !config.models.iter().any(|m| m.id == self.model) {
                        return Err(AppError::Invalid);
                    }
                    if !p.models.contains(&self.model) {
                        p.models.push(self.model.clone());
                    }
                    p.default_model = Some(self.model.clone());
                    p.daily_tenant_micros = self.daily_tenant_micros;
                    p.daily_user_micros = self.daily_user_micros;
                }
                "inherit" => {
                    p.client_models
                        .retain(|c| c.tenant != self.tenant || c.subject.is_some());
                    p.budget_overrides
                        .retain(|b| b.tenant != self.tenant || b.subject.is_some());
                }
                "client" if !self.tenant.trim().is_empty() && self.tenant.len() <= 200 => {
                    p.client_models
                        .retain(|c| c.tenant != self.tenant || c.subject.is_some());
                    p.client_models.push(ClientModels {
                        tenant: self.tenant.clone(),
                        subject: None,
                        models: vec![self.model.clone()],
                        default_model: self.model.clone(),
                        allow_selection: false,
                    });
                    p.budget_overrides
                        .retain(|b| b.tenant != self.tenant || b.subject.is_some());
                    p.budget_overrides.push(BudgetOverride {
                        tenant: self.tenant.clone(),
                        subject: None,
                        daily_micros: self.daily_tenant_micros,
                    });
                }
                _ => return Err(AppError::Invalid),
            }
        }
        config.validate()
    }
}
