//! Data models for ONI buildings and resources

#[derive(Debug, Clone)]
pub struct Resource {
    pub id: String,
    pub name: String,
    pub state: Option<String>, // Solid, Liquid, Gas
    pub specific_heat_capacity: Option<f64>,
    pub thermal_conductivity: Option<f64>,
    pub melt_point_c: Option<f64>,
    pub boil_point_c: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct Building {
    pub id: String,
    pub name: String,
    pub category: Option<String>,
    pub power_watts: f64,       // Negative = consumes, Positive = generates
    pub heat_output_dtu: f64,
    pub construction_time_s: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct BuildingInput {
    pub building_id: String,
    pub resource_id: String,
    pub rate_kg_per_s: f64,
}

#[derive(Debug, Clone)]
pub struct BuildingOutput {
    pub building_id: String,
    pub resource_id: String,
    pub rate_kg_per_s: f64,
}

#[derive(Debug, Clone)]
pub struct Recipe {
    pub id: i64,
    pub building_id: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct RecipeInput {
    pub recipe_id: i64,
    pub resource_id: String,
    pub rate_kg_per_s: f64,
}

#[derive(Debug, Clone)]
pub struct RecipeOutput {
    pub recipe_id: i64,
    pub resource_id: String,
    pub rate_kg_per_s: f64,
}

/// Result of a production chain calculation
#[derive(Debug, Clone)]
pub struct ProductionNode {
    pub building_id: String,
    pub building_name: String,
    pub count: f64,
    pub power_watts: f64,
    pub inputs: Vec<InputRequirement>,
}

#[derive(Debug, Clone)]
pub struct InputRequirement {
    pub resource_id: String,
    pub rate_kg_per_s: f64,
    pub upstream: Option<Box<ProductionNode>>,
}
