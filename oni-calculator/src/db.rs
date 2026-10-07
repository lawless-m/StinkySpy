//! Database schema and operations

use anyhow::Result;
use rusqlite::Connection;

use crate::models::{
    Building, BuildingInput, BuildingOutput,
    Critter, CritterInput, CritterOutput,
    Food,
    Plant, PlantNeed, PlantOutput, PlantAtmosphere,
    Recipe, RecipeInput, RecipeOutput,
};

/// Initialize the database schema
pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        -- Core element/resource data
        CREATE TABLE IF NOT EXISTS resources (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            state TEXT,
            specific_heat_capacity REAL,
            thermal_conductivity REAL,
            melt_point_c REAL,
            boil_point_c REAL
        );

        -- Building definitions
        CREATE TABLE IF NOT EXISTS buildings (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            category TEXT,
            power_watts REAL,
            heat_output_dtu REAL,
            construction_time_s REAL
        );

        -- Building material requirements
        CREATE TABLE IF NOT EXISTS building_materials (
            building_id TEXT,
            resource_id TEXT,
            mass_kg REAL,
            PRIMARY KEY (building_id, resource_id)
        );

        -- Production inputs (what a building consumes)
        CREATE TABLE IF NOT EXISTS building_inputs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            building_id TEXT,
            resource_id TEXT,
            rate_kg_per_s REAL NOT NULL
        );

        -- Production outputs (what a building produces)
        CREATE TABLE IF NOT EXISTS building_outputs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            building_id TEXT,
            resource_id TEXT,
            rate_kg_per_s REAL NOT NULL
        );

        -- Some buildings have multiple operational modes (e.g., Metal Refinery recipes)
        CREATE TABLE IF NOT EXISTS recipes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            building_id TEXT,
            name TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS recipe_inputs (
            recipe_id INTEGER,
            resource_id TEXT,
            rate_kg_per_s REAL NOT NULL,
            PRIMARY KEY (recipe_id, resource_id)
        );

        CREATE TABLE IF NOT EXISTS recipe_outputs (
            recipe_id INTEGER,
            resource_id TEXT,
            rate_kg_per_s REAL NOT NULL,
            PRIMARY KEY (recipe_id, resource_id)
        );

        -- Critters (animals that produce resources)
        CREATE TABLE IF NOT EXISTS critters (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            species TEXT NOT NULL,
            calories_per_cycle REAL NOT NULL,
            kg_per_cycle REAL NOT NULL,
            conversion_efficiency REAL NOT NULL,
            min_poop_kg REAL NOT NULL,
            egg_mass_kg REAL NOT NULL,
            pen_size_tiles INTEGER NOT NULL DEFAULT 12
        );

        CREATE TABLE IF NOT EXISTS critter_inputs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            critter_id TEXT NOT NULL,
            resource_id TEXT NOT NULL,
            food_type TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS critter_outputs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            critter_id TEXT NOT NULL,
            resource_id TEXT NOT NULL,
            rate_kg_per_cycle REAL NOT NULL
        );

        -- Plants (crops that require irrigation/fertilizer)
        CREATE TABLE IF NOT EXISTS plants (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            growth_duration_cycles REAL NOT NULL,
            temp_lethal_low REAL NOT NULL,
            temp_warning_low REAL NOT NULL,
            temp_warning_high REAL NOT NULL,
            temp_lethal_high REAL NOT NULL
        );

        CREATE TABLE IF NOT EXISTS plant_irrigation (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            plant_id TEXT NOT NULL,
            resource_id TEXT NOT NULL,
            rate_kg_per_s REAL NOT NULL
        );

        CREATE TABLE IF NOT EXISTS plant_fertilizer (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            plant_id TEXT NOT NULL,
            resource_id TEXT NOT NULL,
            rate_kg_per_s REAL NOT NULL
        );

        CREATE TABLE IF NOT EXISTS plant_outputs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            plant_id TEXT NOT NULL,
            crop_id TEXT NOT NULL,
            num_produced INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS plant_atmosphere (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            plant_id TEXT NOT NULL,
            resource_id TEXT NOT NULL
        );

        -- Food items (edibles)
        CREATE TABLE IF NOT EXISTS foods (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            calories REAL NOT NULL,
            quality INTEGER NOT NULL,
            preserve_temp_c REAL NOT NULL,
            spoil_temp_c REAL NOT NULL,
            spoil_time_s REAL NOT NULL
        );

        -- Create indexes for common lookups
        CREATE INDEX IF NOT EXISTS idx_building_inputs_building ON building_inputs(building_id);
        CREATE INDEX IF NOT EXISTS idx_building_outputs_building ON building_outputs(building_id);
        CREATE INDEX IF NOT EXISTS idx_building_outputs_resource ON building_outputs(resource_id);
        "#,
    )?;
    Ok(())
}

/// Insert or replace a building
pub fn upsert_building(conn: &Connection, building: &Building) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO buildings (id, name, category, power_watts, heat_output_dtu, construction_time_s)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        (
            &building.id,
            &building.name,
            &building.category,
            building.power_watts,
            building.heat_output_dtu,
            building.construction_time_s,
        ),
    )?;
    Ok(())
}

/// Insert a building input
pub fn insert_building_input(conn: &Connection, input: &BuildingInput) -> Result<()> {
    conn.execute(
        "INSERT INTO building_inputs (building_id, resource_id, rate_kg_per_s)
         VALUES (?1, ?2, ?3)",
        (&input.building_id, &input.resource_id, input.rate_kg_per_s),
    )?;
    Ok(())
}

/// Insert a building output
pub fn insert_building_output(conn: &Connection, output: &BuildingOutput) -> Result<()> {
    conn.execute(
        "INSERT INTO building_outputs (building_id, resource_id, rate_kg_per_s)
         VALUES (?1, ?2, ?3)",
        (&output.building_id, &output.resource_id, output.rate_kg_per_s),
    )?;
    Ok(())
}

/// Clear all extracted data (for re-extraction)
pub fn clear_extracted_data(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        DELETE FROM recipe_outputs;
        DELETE FROM recipe_inputs;
        DELETE FROM recipes;
        DELETE FROM building_outputs;
        DELETE FROM building_inputs;
        DELETE FROM building_materials;
        DELETE FROM buildings;
        DELETE FROM resources;
        DELETE FROM critter_outputs;
        DELETE FROM critter_inputs;
        DELETE FROM critters;
        DELETE FROM plant_atmosphere;
        DELETE FROM plant_outputs;
        DELETE FROM plant_fertilizer;
        DELETE FROM plant_irrigation;
        DELETE FROM plants;
        DELETE FROM foods;
        "#,
    )?;
    Ok(())
}

/// Get all buildings that produce a given resource
pub fn get_producers(conn: &Connection, resource_id: &str) -> Result<Vec<(Building, f64)>> {
    let mut stmt = conn.prepare(
        "SELECT b.id, b.name, b.category, b.power_watts, b.heat_output_dtu, b.construction_time_s, bo.rate_kg_per_s
         FROM buildings b
         JOIN building_outputs bo ON b.id = bo.building_id
         WHERE bo.resource_id = ?1",
    )?;

    let rows = stmt.query_map([resource_id], |row| {
        Ok((
            Building {
                id: row.get(0)?,
                name: row.get(1)?,
                category: row.get(2)?,
                power_watts: row.get(3)?,
                heat_output_dtu: row.get(4)?,
                construction_time_s: row.get(5)?,
            },
            row.get::<_, f64>(6)?,
        ))
    })?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

/// Get all inputs for a building
pub fn get_building_inputs(conn: &Connection, building_id: &str) -> Result<Vec<BuildingInput>> {
    let mut stmt = conn.prepare(
        "SELECT building_id, resource_id, rate_kg_per_s
         FROM building_inputs
         WHERE building_id = ?1",
    )?;

    let rows = stmt.query_map([building_id], |row| {
        Ok(BuildingInput {
            building_id: row.get(0)?,
            resource_id: row.get(1)?,
            rate_kg_per_s: row.get(2)?,
        })
    })?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

/// List all buildings in the database
pub fn list_buildings(conn: &Connection) -> Result<Vec<Building>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, category, power_watts, heat_output_dtu, construction_time_s FROM buildings ORDER BY name",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(Building {
            id: row.get(0)?,
            name: row.get(1)?,
            category: row.get(2)?,
            power_watts: row.get(3)?,
            heat_output_dtu: row.get(4)?,
            construction_time_s: row.get(5)?,
        })
    })?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

/// List all unique resources that are outputs
pub fn list_producible_resources(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT resource_id FROM building_outputs ORDER BY resource_id",
    )?;

    let rows = stmt.query_map([], |row| row.get(0))?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

// ========== Critter Operations ==========

/// Insert or replace a critter
pub fn upsert_critter(conn: &Connection, critter: &Critter) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO critters (id, name, species, calories_per_cycle, kg_per_cycle, conversion_efficiency, min_poop_kg, egg_mass_kg, pen_size_tiles)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        (
            &critter.id,
            &critter.name,
            &critter.species,
            critter.calories_per_cycle,
            critter.kg_per_cycle,
            critter.conversion_efficiency,
            critter.min_poop_kg,
            critter.egg_mass_kg,
            critter.pen_size_tiles,
        ),
    )?;
    Ok(())
}

/// Insert a critter input (food)
pub fn insert_critter_input(conn: &Connection, input: &CritterInput) -> Result<()> {
    conn.execute(
        "INSERT INTO critter_inputs (critter_id, resource_id, food_type)
         VALUES (?1, ?2, ?3)",
        (&input.critter_id, &input.resource_id, &input.food_type),
    )?;
    Ok(())
}

/// Insert a critter output (poop/drops)
pub fn insert_critter_output(conn: &Connection, output: &CritterOutput) -> Result<()> {
    conn.execute(
        "INSERT INTO critter_outputs (critter_id, resource_id, rate_kg_per_cycle)
         VALUES (?1, ?2, ?3)",
        (&output.critter_id, &output.resource_id, output.rate_kg_per_cycle),
    )?;
    Ok(())
}

/// List all critters in the database
pub fn list_critters(conn: &Connection) -> Result<Vec<Critter>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, species, calories_per_cycle, kg_per_cycle, conversion_efficiency, min_poop_kg, egg_mass_kg, pen_size_tiles
         FROM critters ORDER BY species, name",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(Critter {
            id: row.get(0)?,
            name: row.get(1)?,
            species: row.get(2)?,
            calories_per_cycle: row.get(3)?,
            kg_per_cycle: row.get(4)?,
            conversion_efficiency: row.get(5)?,
            min_poop_kg: row.get(6)?,
            egg_mass_kg: row.get(7)?,
            pen_size_tiles: row.get(8)?,
        })
    })?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

/// Get critter inputs (food requirements)
pub fn get_critter_inputs(conn: &Connection, critter_id: &str) -> Result<Vec<CritterInput>> {
    let mut stmt = conn.prepare(
        "SELECT critter_id, resource_id, food_type
         FROM critter_inputs
         WHERE critter_id = ?1",
    )?;

    let rows = stmt.query_map([critter_id], |row| {
        Ok(CritterInput {
            critter_id: row.get(0)?,
            resource_id: row.get(1)?,
            food_type: row.get(2)?,
        })
    })?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

/// Get critter outputs (what they produce)
pub fn get_critter_outputs(conn: &Connection, critter_id: &str) -> Result<Vec<CritterOutput>> {
    let mut stmt = conn.prepare(
        "SELECT critter_id, resource_id, rate_kg_per_cycle
         FROM critter_outputs
         WHERE critter_id = ?1",
    )?;

    let rows = stmt.query_map([critter_id], |row| {
        Ok(CritterOutput {
            critter_id: row.get(0)?,
            resource_id: row.get(1)?,
            rate_kg_per_cycle: row.get(2)?,
        })
    })?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

// ========== Plant Operations ==========

/// Insert or replace a plant
pub fn upsert_plant(conn: &Connection, plant: &Plant) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO plants (id, name, growth_duration_cycles, temp_lethal_low, temp_warning_low, temp_warning_high, temp_lethal_high)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        (
            &plant.id,
            &plant.name,
            plant.growth_duration_cycles,
            plant.temp_lethal_low,
            plant.temp_warning_low,
            plant.temp_warning_high,
            plant.temp_lethal_high,
        ),
    )?;
    Ok(())
}

/// Insert a plant irrigation requirement
pub fn insert_plant_irrigation(conn: &Connection, need: &PlantNeed) -> Result<()> {
    conn.execute(
        "INSERT INTO plant_irrigation (plant_id, resource_id, rate_kg_per_s)
         VALUES (?1, ?2, ?3)",
        (&need.plant_id, &need.resource_id, need.rate_kg_per_s),
    )?;
    Ok(())
}

/// Insert a plant fertilizer requirement
pub fn insert_plant_fertilizer(conn: &Connection, need: &PlantNeed) -> Result<()> {
    conn.execute(
        "INSERT INTO plant_fertilizer (plant_id, resource_id, rate_kg_per_s)
         VALUES (?1, ?2, ?3)",
        (&need.plant_id, &need.resource_id, need.rate_kg_per_s),
    )?;
    Ok(())
}

/// Insert a plant output (harvest)
pub fn insert_plant_output(conn: &Connection, output: &PlantOutput) -> Result<()> {
    conn.execute(
        "INSERT INTO plant_outputs (plant_id, crop_id, num_produced)
         VALUES (?1, ?2, ?3)",
        (&output.plant_id, &output.crop_id, output.num_produced),
    )?;
    Ok(())
}

/// Insert a plant atmosphere requirement
pub fn insert_plant_atmosphere(conn: &Connection, atmo: &PlantAtmosphere) -> Result<()> {
    conn.execute(
        "INSERT INTO plant_atmosphere (plant_id, resource_id)
         VALUES (?1, ?2)",
        (&atmo.plant_id, &atmo.resource_id),
    )?;
    Ok(())
}

/// List all plants in the database
pub fn list_plants(conn: &Connection) -> Result<Vec<Plant>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, growth_duration_cycles, temp_lethal_low, temp_warning_low, temp_warning_high, temp_lethal_high
         FROM plants ORDER BY name",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(Plant {
            id: row.get(0)?,
            name: row.get(1)?,
            growth_duration_cycles: row.get(2)?,
            temp_lethal_low: row.get(3)?,
            temp_warning_low: row.get(4)?,
            temp_warning_high: row.get(5)?,
            temp_lethal_high: row.get(6)?,
        })
    })?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

// ========== Food Operations ==========

/// Insert or replace a food item
pub fn upsert_food(conn: &Connection, food: &Food) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO foods (id, name, calories, quality, preserve_temp_c, spoil_temp_c, spoil_time_s)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        (
            &food.id,
            &food.name,
            food.calories,
            food.quality,
            food.preserve_temp_c,
            food.spoil_temp_c,
            food.spoil_time_s,
        ),
    )?;
    Ok(())
}

/// List all food items in the database
pub fn list_foods(conn: &Connection) -> Result<Vec<Food>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, calories, quality, preserve_temp_c, spoil_temp_c, spoil_time_s
         FROM foods ORDER BY name",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(Food {
            id: row.get(0)?,
            name: row.get(1)?,
            calories: row.get(2)?,
            quality: row.get(3)?,
            preserve_temp_c: row.get(4)?,
            spoil_temp_c: row.get(5)?,
            spoil_time_s: row.get(6)?,
        })
    })?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

// ========== Recipe Operations ==========

/// Insert or replace a recipe
pub fn upsert_recipe(conn: &Connection, recipe: &Recipe) -> Result<i64> {
    conn.execute(
        "INSERT OR REPLACE INTO recipes (id, building_id, name)
         VALUES (?1, ?2, ?3)",
        (recipe.id, &recipe.building_id, &recipe.name),
    )?;
    Ok(conn.last_insert_rowid())
}

/// Insert a new recipe with auto-generated ID
pub fn insert_recipe(conn: &Connection, building_id: &str, name: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO recipes (building_id, name) VALUES (?1, ?2)",
        (building_id, name),
    )?;
    Ok(conn.last_insert_rowid())
}

/// Insert a recipe input
pub fn insert_recipe_input(conn: &Connection, input: &RecipeInput) -> Result<()> {
    conn.execute(
        "INSERT INTO recipe_inputs (recipe_id, resource_id, rate_kg_per_s)
         VALUES (?1, ?2, ?3)",
        (input.recipe_id, &input.resource_id, input.rate_kg_per_s),
    )?;
    Ok(())
}

/// Insert a recipe output
pub fn insert_recipe_output(conn: &Connection, output: &RecipeOutput) -> Result<()> {
    conn.execute(
        "INSERT INTO recipe_outputs (recipe_id, resource_id, rate_kg_per_s)
         VALUES (?1, ?2, ?3)",
        (output.recipe_id, &output.resource_id, output.rate_kg_per_s),
    )?;
    Ok(())
}

/// List all recipes in the database
pub fn list_recipes(conn: &Connection) -> Result<Vec<Recipe>> {
    let mut stmt = conn.prepare(
        "SELECT id, building_id, name FROM recipes ORDER BY building_id, name",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(Recipe {
            id: row.get(0)?,
            building_id: row.get(1)?,
            name: row.get(2)?,
        })
    })?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}
