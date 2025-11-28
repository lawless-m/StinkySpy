//! Database schema and operations

use anyhow::Result;
use rusqlite::Connection;

use crate::models::{Building, BuildingInput, BuildingOutput};

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
