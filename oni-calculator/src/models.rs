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

// ========== Critters ==========

#[derive(Debug, Clone)]
pub struct Critter {
    pub id: String,
    pub name: String,
    pub species: String,
    pub calories_per_cycle: f64,
    pub kg_per_cycle: f64,
    pub conversion_efficiency: f64,
    pub min_poop_kg: f64,
    pub egg_mass_kg: f64,
    pub pen_size_tiles: i32,  // Tiles required per creature in stable
}

#[derive(Debug, Clone)]
pub struct CritterInput {
    pub critter_id: String,
    pub resource_id: String,
    pub food_type: String,
}

#[derive(Debug, Clone)]
pub struct CritterOutput {
    pub critter_id: String,
    pub resource_id: String,
    pub rate_kg_per_cycle: f64,
}

// ========== Food ==========

#[derive(Debug, Clone)]
pub struct Food {
    pub id: String,
    pub name: String,
    pub calories: f64,
    pub quality: i32,  // Morale bonus (-1 = bad, 0 = neutral, 1+ = good)
    pub preserve_temp_c: f64,
    pub spoil_temp_c: f64,
    pub spoil_time_s: f64,
}

// ========== Plants ==========

#[derive(Debug, Clone)]
pub struct Plant {
    pub id: String,
    pub name: String,
    pub growth_duration_cycles: f64,
    pub temp_lethal_low: f64,
    pub temp_warning_low: f64,
    pub temp_warning_high: f64,
    pub temp_lethal_high: f64,
}

#[derive(Debug, Clone)]
pub struct PlantNeed {
    pub plant_id: String,
    pub resource_id: String,
    pub rate_kg_per_s: f64,
}

#[derive(Debug, Clone)]
pub struct PlantOutput {
    pub plant_id: String,
    pub crop_id: String,
    pub num_produced: i32,
}

#[derive(Debug, Clone)]
pub struct PlantAtmosphere {
    pub plant_id: String,
    pub resource_id: String,
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
