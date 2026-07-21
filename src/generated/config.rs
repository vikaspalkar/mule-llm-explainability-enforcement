use serde::Deserialize;
#[derive(Deserialize, Clone, Debug)]
pub struct ExplainabilityFields0Config {
    #[serde(alias = "allowed_values")]
    pub allowed_values: Option<Vec<String>>,
    #[serde(alias = "description")]
    pub description: Option<String>,
    #[serde(alias = "field")]
    pub field: String,
    #[serde(alias = "field_type")]
    pub field_type: String,
    #[serde(alias = "required")]
    pub required: Option<bool>,
    #[serde(alias = "required_when_equals")]
    pub required_when_equals: Option<String>,
    #[serde(alias = "required_when_field")]
    pub required_when_field: Option<String>,
    #[serde(alias = "validation_max")]
    pub validation_max: Option<f64>,
    #[serde(alias = "validation_min")]
    pub validation_min: Option<f64>,
}
#[derive(Deserialize, Clone, Debug)]
pub struct Config {
    #[serde(alias = "explainability_fields")]
    pub explainability_fields: Vec<ExplainabilityFields0Config>,
    #[serde(alias = "prompt_custom_preamble")]
    pub prompt_custom_preamble: Option<String>,
    #[serde(alias = "scope_enforce_for_paths")]
    pub scope_enforce_for_paths: Option<Vec<String>>,
    #[serde(alias = "scope_response_validation_enabled")]
    pub scope_response_validation_enabled: Option<bool>,
    #[serde(alias = "validation_minimum_compliance_percentage")]
    pub validation_minimum_compliance_percentage: Option<i64>,
    #[serde(alias = "validation_on_failure")]
    pub validation_on_failure: Option<String>,
}
#[pdk::hl::entrypoint_flex]
fn init(abi: &dyn pdk::flex_abi::api::FlexAbi) -> Result<(), anyhow::Error> {
    abi.setup()?;
    Ok(())
}
