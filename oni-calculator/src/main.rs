//! ONI Production Calculator
//!
//! A production chain calculator for Oxygen Not Included.

mod calculator;
mod db;
mod extract;
mod models;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use rusqlite::Connection;

#[derive(Parser)]
#[command(name = "oni-calculator")]
#[command(about = "Production chain calculator for Oxygen Not Included")]
struct Cli {
    /// Path to the SQLite database
    #[arg(short, long, default_value = "/home/matt/Git/StinkySpy/oni_data.db")]
    database: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Extract building data from decompiled C# source
    Extract {
        /// Path to decompiled source directory
        source_dir: PathBuf,

        /// Clear existing data before extraction
        #[arg(long)]
        clear: bool,
    },

    /// Calculate production chain for a target resource
    Calc {
        /// Target resource to produce (e.g., "Oxygen", "Steel")
        resource: String,

        /// Target production rate in kg/s
        #[arg(short, long, default_value = "1.0")]
        rate: f64,

        /// Show detailed production tree
        #[arg(short, long)]
        verbose: bool,
    },

    /// List all buildings in the database
    ListBuildings,

    /// List all producible resources
    ListResources,

    /// Show details for a specific building
    Building {
        /// Building ID
        id: String,
    },

    /// List all critters in the database
    ListCritters,

    /// Show details for a specific critter
    Critter {
        /// Critter ID
        id: String,
    },

    /// List all recipes in the database
    ListRecipes,

    /// Show details for a specific recipe
    Recipe {
        /// Recipe ID
        id: String,
    },

    /// List all plants in the database
    ListPlants,

    /// Show details for a specific plant
    Plant {
        /// Plant ID
        id: String,
    },

    /// List all food items in the database
    ListFoods,

    /// Show details for a specific food item
    Food {
        /// Food ID
        id: String,
    },

    /// Initialize empty database with schema
    Init,

    /// Load sample data for testing (without decompiled source)
    LoadSample,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let conn = Connection::open(&cli.database)?;
    db::init_schema(&conn)?;

    match cli.command {
        Commands::Extract { source_dir, clear } => {
            if clear {
                println!("Clearing existing data...");
                db::clear_extracted_data(&conn)?;
            }

            let stats = extract::extract_to_database(&conn, &source_dir)?;
            println!("\n{}", stats);
        }

        Commands::Calc {
            resource,
            rate,
            verbose,
        } => {
            let chain = calculator::calculate_production_chain(&conn, &resource, rate)?;

            if verbose {
                println!("Production chain:\n");
                println!("{}", calculator::format_production_chain(&chain, 0));
            }

            let summary = calculator::summarize_chain(&chain, &resource, rate);
            println!("{}", summary);
        }

        Commands::ListBuildings => {
            let buildings = db::list_buildings(&conn)?;
            if buildings.is_empty() {
                println!("No buildings in database. Run 'extract' or 'load-sample' first.");
            } else {
                println!("{:<30} {:>10} {:>10}", "Building", "Power (W)", "Heat (DTU/s)");
                println!("{}", "-".repeat(52));
                for b in buildings {
                    println!("{:<30} {:>10.0} {:>10.0}", b.name, b.power_watts, b.heat_output_dtu);
                }
            }
        }

        Commands::ListResources => {
            let resources = db::list_producible_resources(&conn)?;
            if resources.is_empty() {
                println!("No resources in database. Run 'extract' or 'load-sample' first.");
            } else {
                println!("Producible resources:");
                for r in resources {
                    println!("  {}", r);
                }
            }
        }

        Commands::Building { id } => {
            let buildings = db::list_buildings(&conn)?;
            if let Some(b) = buildings.iter().find(|b| b.id == id) {
                println!("Building: {}", b.name);
                println!("  ID: {}", b.id);
                println!("  Power: {}W", b.power_watts);
                println!("  Heat: {} DTU/s", b.heat_output_dtu);

                let inputs = db::get_building_inputs(&conn, &id)?;
                if !inputs.is_empty() {
                    println!("  Inputs:");
                    for i in inputs {
                        println!("    {} @ {} kg/s", i.resource_id, i.rate_kg_per_s);
                    }
                }

                let producers = db::get_producers(&conn, &id)?;
                // This is backwards - let's get outputs instead
                // For now, just query directly
                let mut stmt = conn.prepare(
                    "SELECT resource_id, rate_kg_per_s FROM building_outputs WHERE building_id = ?",
                )?;
                let outputs: Vec<(String, f64)> = stmt
                    .query_map([&id], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .filter_map(|r| r.ok())
                    .collect();

                if !outputs.is_empty() {
                    println!("  Outputs:");
                    for (res, rate) in outputs {
                        println!("    {} @ {} kg/s", res, rate);
                    }
                }
            } else {
                println!("Building '{}' not found", id);
            }
        }

        Commands::ListCritters => {
            let critters = db::list_critters(&conn)?;
            if critters.is_empty() {
                println!("No critters in database. Run 'extract' first.");
            } else {
                println!("{:<20} {:<15} {:>12} {:>10}", "Critter", "Species", "Food (kg/c)", "Eff");
                println!("{}", "-".repeat(62));
                for c in critters {
                    println!("{:<20} {:<15} {:>12.1} {:>10.0}%",
                        c.name, c.species, c.kg_per_cycle, c.conversion_efficiency * 100.0);
                }
            }
        }

        Commands::Critter { id } => {
            let critters = db::list_critters(&conn)?;
            if let Some(c) = critters.iter().find(|c| c.id == id) {
                println!("Critter: {}", c.name);
                println!("  ID: {}", c.id);
                println!("  Species: {}", c.species);
                println!("  Calories per cycle: {}", c.calories_per_cycle);
                println!("  Food consumption: {} kg/cycle", c.kg_per_cycle);
                println!("  Conversion efficiency: {:.0}%", c.conversion_efficiency * 100.0);
                println!("  Min poop size: {} kg", c.min_poop_kg);
                println!("  Egg mass: {} kg", c.egg_mass_kg);
                println!("  Pen size: {} tiles per creature", c.pen_size_tiles);

                // Calculate stable capacity (max 96 tiles for Ranch Station bonus)
                let max_stable_size = 96;
                let max_per_stable = max_stable_size / c.pen_size_tiles;
                println!("  Stable capacity: {} critters (in {} tiles)", max_per_stable, max_stable_size);

                let inputs = db::get_critter_inputs(&conn, &id)?;
                if !inputs.is_empty() {
                    println!("  Diet (can eat):");
                    for i in inputs {
                        if i.resource_id == "ALL_FOODS" {
                            println!("    All prepared foods (FoodDiet)");
                        } else {
                            println!("    {} ({})", i.resource_id, i.food_type);
                        }
                    }
                }

                let outputs = db::get_critter_outputs(&conn, &id)?;
                if !outputs.is_empty() {
                    println!("  Outputs:");
                    for o in outputs {
                        println!("    {} @ {} kg/cycle", o.resource_id, o.rate_kg_per_cycle);
                    }
                }
            } else {
                println!("Critter '{}' not found", id);
            }
        }

        Commands::ListRecipes => {
            let recipes = db::list_recipes(&conn)?;
            if recipes.is_empty() {
                println!("No recipes in database. Run 'extract' first.");
            } else {
                println!("{:<30} {:<25}", "Recipe", "Building");
                println!("{}", "-".repeat(57));
                for r in recipes {
                    println!("{:<30} {:<25}", r.name, r.building_id);
                }
            }
        }

        Commands::Recipe { id } => {
            println!("Recipe details not yet implemented for '{}'", id);
            println!("Use 'list-recipes' to see all available recipes.");
        }

        Commands::ListPlants => {
            let plants = db::list_plants(&conn)?;
            if plants.is_empty() {
                println!("No plants in database. Run 'extract' first.");
            } else {
                println!("{:<25} {:>15}", "Plant", "Growth (cycles)");
                println!("{}", "-".repeat(42));
                for p in plants {
                    println!("{:<25} {:>15.1}", p.name, p.growth_duration_cycles);
                }
            }
        }

        Commands::Plant { id } => {
            let plants = db::list_plants(&conn)?;
            if let Some(p) = plants.iter().find(|p| p.id == id) {
                println!("Plant: {}", p.name);
                println!("  ID: {}", p.id);
                println!("  Growth duration: {} cycles", p.growth_duration_cycles);
                println!("  Temperature range:");
                println!("    Lethal low: {:.1}K", p.temp_lethal_low);
                println!("    Warning low: {:.1}K", p.temp_warning_low);
                println!("    Warning high: {:.1}K", p.temp_warning_high);
                println!("    Lethal high: {:.1}K", p.temp_lethal_high);
            } else {
                println!("Plant '{}' not found", id);
            }
        }

        Commands::ListFoods => {
            let foods = db::list_foods(&conn)?;
            if foods.is_empty() {
                println!("No foods in database. Run 'extract' first.");
            } else {
                println!("{:<25} {:>12} {:>8}", "Food", "Calories", "Quality");
                println!("{}", "-".repeat(48));
                for f in foods {
                    let quality_str = match f.quality {
                        q if q < 0 => format!("{}", q),
                        0 => "0".to_string(),
                        q => format!("+{}", q),
                    };
                    println!("{:<25} {:>12.0} {:>8}", f.name, f.calories, quality_str);
                }
            }
        }

        Commands::Food { id } => {
            let foods = db::list_foods(&conn)?;
            if let Some(f) = foods.iter().find(|f| f.id == id) {
                println!("Food: {}", f.name);
                println!("  ID: {}", f.id);
                println!("  Calories: {:.0}", f.calories);
                println!("  Quality: {}", f.quality);
                println!("  Preservation temp: {:.1}°C", f.preserve_temp_c);
                println!("  Spoil temp: {:.1}°C", f.spoil_temp_c);
                println!("  Spoil time: {:.0}s ({:.1} cycles)", f.spoil_time_s, f.spoil_time_s / 600.0);
            } else {
                println!("Food '{}' not found", id);
            }
        }

        Commands::Init => {
            println!("Database initialized at: {}", cli.database.display());
        }

        Commands::LoadSample => {
            load_sample_data(&conn)?;
            println!("Sample data loaded successfully!");
        }
    }

    Ok(())
}

/// Load sample ONI building data for testing without decompiled source
fn load_sample_data(conn: &Connection) -> Result<()> {
    use crate::models::{Building, BuildingInput, BuildingOutput};

    db::clear_extracted_data(conn)?;

    // Electrolyzer: Water -> Oxygen + Hydrogen
    let electrolyzer = Building {
        id: "Electrolyzer".to_string(),
        name: "Electrolyzer".to_string(),
        category: Some("Oxygen".to_string()),
        power_watts: -120.0,
        heat_output_dtu: 1000.0,
        construction_time_s: Some(30.0),
    };
    db::upsert_building(conn, &electrolyzer)?;
    db::insert_building_input(
        conn,
        &BuildingInput {
            building_id: "Electrolyzer".to_string(),
            resource_id: "Water".to_string(),
            rate_kg_per_s: 1.0,
        },
    )?;
    db::insert_building_output(
        conn,
        &BuildingOutput {
            building_id: "Electrolyzer".to_string(),
            resource_id: "Oxygen".to_string(),
            rate_kg_per_s: 0.888,
        },
    )?;
    db::insert_building_output(
        conn,
        &BuildingOutput {
            building_id: "Electrolyzer".to_string(),
            resource_id: "Hydrogen".to_string(),
            rate_kg_per_s: 0.112,
        },
    )?;

    // Hydrogen Generator: Hydrogen -> Power
    let h2_gen = Building {
        id: "HydrogenGenerator".to_string(),
        name: "Hydrogen Generator".to_string(),
        category: Some("Power".to_string()),
        power_watts: 800.0, // Generates power
        heat_output_dtu: 2000.0,
        construction_time_s: Some(120.0),
    };
    db::upsert_building(conn, &h2_gen)?;
    db::insert_building_input(
        conn,
        &BuildingInput {
            building_id: "HydrogenGenerator".to_string(),
            resource_id: "Hydrogen".to_string(),
            rate_kg_per_s: 0.1,
        },
    )?;

    // Coal Generator
    let coal_gen = Building {
        id: "Generator".to_string(),
        name: "Coal Generator".to_string(),
        category: Some("Power".to_string()),
        power_watts: 600.0,
        heat_output_dtu: 9000.0,
        construction_time_s: Some(120.0),
    };
    db::upsert_building(conn, &coal_gen)?;
    db::insert_building_input(
        conn,
        &BuildingInput {
            building_id: "Generator".to_string(),
            resource_id: "Coal".to_string(),
            rate_kg_per_s: 1.0,
        },
    )?;
    db::insert_building_output(
        conn,
        &BuildingOutput {
            building_id: "Generator".to_string(),
            resource_id: "CarbonDioxide".to_string(),
            rate_kg_per_s: 0.02,
        },
    )?;

    // Water Sieve: Polluted Water -> Water + Polluted Dirt
    let sieve = Building {
        id: "WaterPurifier".to_string(),
        name: "Water Sieve".to_string(),
        category: Some("Plumbing".to_string()),
        power_watts: -120.0,
        heat_output_dtu: 500.0,
        construction_time_s: Some(30.0),
    };
    db::upsert_building(conn, &sieve)?;
    db::insert_building_input(
        conn,
        &BuildingInput {
            building_id: "WaterPurifier".to_string(),
            resource_id: "DirtyWater".to_string(),
            rate_kg_per_s: 5.0,
        },
    )?;
    db::insert_building_input(
        conn,
        &BuildingInput {
            building_id: "WaterPurifier".to_string(),
            resource_id: "Sand".to_string(),
            rate_kg_per_s: 1.0,
        },
    )?;
    db::insert_building_output(
        conn,
        &BuildingOutput {
            building_id: "WaterPurifier".to_string(),
            resource_id: "Water".to_string(),
            rate_kg_per_s: 5.0,
        },
    )?;
    db::insert_building_output(
        conn,
        &BuildingOutput {
            building_id: "WaterPurifier".to_string(),
            resource_id: "ToxicSand".to_string(),
            rate_kg_per_s: 0.2,
        },
    )?;

    // Metal Refinery: Ore + Coolant -> Refined Metal
    let refinery = Building {
        id: "MetalRefinery".to_string(),
        name: "Metal Refinery".to_string(),
        category: Some("Refining".to_string()),
        power_watts: -1200.0,
        heat_output_dtu: 16000.0,
        construction_time_s: Some(120.0),
    };
    db::upsert_building(conn, &refinery)?;
    db::insert_building_input(
        conn,
        &BuildingInput {
            building_id: "MetalRefinery".to_string(),
            resource_id: "IronOre".to_string(),
            rate_kg_per_s: 0.5, // 100kg per 200s cycle = 0.5 kg/s avg
        },
    )?;
    db::insert_building_output(
        conn,
        &BuildingOutput {
            building_id: "MetalRefinery".to_string(),
            resource_id: "Iron".to_string(),
            rate_kg_per_s: 0.5,
        },
    )?;

    // Algae Terrarium: Water + Algae -> Oxygen + DirtyWater
    let terrarium = Building {
        id: "AlgaeHabitat".to_string(),
        name: "Algae Terrarium".to_string(),
        category: Some("Oxygen".to_string()),
        power_watts: 0.0, // No power required
        heat_output_dtu: -667.0, // Cools!
        construction_time_s: Some(30.0),
    };
    db::upsert_building(conn, &terrarium)?;
    db::insert_building_input(
        conn,
        &BuildingInput {
            building_id: "AlgaeHabitat".to_string(),
            resource_id: "Algae".to_string(),
            rate_kg_per_s: 0.030,
        },
    )?;
    db::insert_building_input(
        conn,
        &BuildingInput {
            building_id: "AlgaeHabitat".to_string(),
            resource_id: "Water".to_string(),
            rate_kg_per_s: 0.300,
        },
    )?;
    db::insert_building_output(
        conn,
        &BuildingOutput {
            building_id: "AlgaeHabitat".to_string(),
            resource_id: "Oxygen".to_string(),
            rate_kg_per_s: 0.040,
        },
    )?;
    db::insert_building_output(
        conn,
        &BuildingOutput {
            building_id: "AlgaeHabitat".to_string(),
            resource_id: "DirtyWater".to_string(),
            rate_kg_per_s: 0.290,
        },
    )?;

    // Natural Gas Generator
    let natgas_gen = Building {
        id: "MethaneGenerator".to_string(),
        name: "Natural Gas Generator".to_string(),
        category: Some("Power".to_string()),
        power_watts: 800.0,
        heat_output_dtu: 10000.0,
        construction_time_s: Some(120.0),
    };
    db::upsert_building(conn, &natgas_gen)?;
    db::insert_building_input(
        conn,
        &BuildingInput {
            building_id: "MethaneGenerator".to_string(),
            resource_id: "Methane".to_string(),
            rate_kg_per_s: 0.090,
        },
    )?;
    db::insert_building_output(
        conn,
        &BuildingOutput {
            building_id: "MethaneGenerator".to_string(),
            resource_id: "CarbonDioxide".to_string(),
            rate_kg_per_s: 0.0225,
        },
    )?;
    db::insert_building_output(
        conn,
        &BuildingOutput {
            building_id: "MethaneGenerator".to_string(),
            resource_id: "DirtyWater".to_string(),
            rate_kg_per_s: 0.0675,
        },
    )?;

    println!("Loaded {} sample buildings", 7);
    Ok(())
}
