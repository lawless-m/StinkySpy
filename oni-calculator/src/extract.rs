//! C# source code extraction for ONI building data
//!
//! Parses decompiled C# source from Assembly-CSharp.dll to extract
//! building definitions, inputs, outputs, and power requirements.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use regex::Regex;
use rusqlite::Connection;
use walkdir::WalkDir;

use crate::db;
use crate::models::{Building, BuildingInput, BuildingOutput};

/// Extracted building data before database insertion
#[derive(Debug, Default)]
struct ExtractedBuilding {
    id: String,
    power_watts: f64,
    heat_dtu: f64,
    inputs: Vec<(String, f64)>,  // (element, rate_kg_s)
    outputs: Vec<(String, f64)>, // (element, rate_kg_s)
}

/// Find all *Config.cs files that likely define buildings
pub fn find_config_files(decompiled_dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    let mut configs = Vec::new();

    for entry in WalkDir::new(decompiled_dir)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.extension().map_or(false, |ext| ext == "cs") {
            let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if filename.ends_with("Config.cs") {
                let content = fs::read_to_string(path).unwrap_or_default();
                if content.contains("IBuildingConfig") || content.contains("CreateBuildingDef") {
                    configs.push(path.to_path_buf());
                }
            }
        }
    }

    Ok(configs)
}

/// Parse a single building config file
fn parse_building_config(filepath: &Path) -> Result<Option<ExtractedBuilding>> {
    let content = fs::read_to_string(filepath)
        .with_context(|| format!("Failed to read {}", filepath.display()))?;

    let mut building = ExtractedBuilding::default();

    // Extract building ID - multiple patterns

    // Pattern 1: string text = "Electrolyzer"; ... CreateBuildingDef(text, ...)
    // Look for: public const string ID = "BuildingID"
    let const_id_re = Regex::new(r#"(?:public\s+)?const\s+string\s+ID\s*=\s*"(\w+)""#)?;
    if let Some(cap) = const_id_re.captures(&content) {
        building.id = cap[1].to_string();
    }

    // Pattern 2: string text = "BuildingID"; at start of CreateBuildingDef method
    if building.id.is_empty() {
        let text_id_re = Regex::new(r#"string\s+text\s*=\s*"(\w+)""#)?;
        if let Some(cap) = text_id_re.captures(&content) {
            building.id = cap[1].to_string();
        }
    }

    // Pattern 3: Direct string in CreateBuildingDef("BuildingID", ...)
    if building.id.is_empty() {
        let direct_id_re = Regex::new(r#"CreateBuildingDef\s*\(\s*"(\w+)""#)?;
        if let Some(cap) = direct_id_re.captures(&content) {
            building.id = cap[1].to_string();
        }
    }

    if building.id.is_empty() {
        return Ok(None);
    }

    // Extract power consumption
    // Pattern: EnergyConsumptionWhenActive = 120f
    let power_re = Regex::new(r"EnergyConsumptionWhenActive\s*=\s*([\d.]+)f?")?;
    if let Some(cap) = power_re.captures(&content) {
        building.power_watts = -cap[1].parse::<f64>().unwrap_or(0.0); // Negative = consumption
    }

    // Check for power generation
    // Pattern: GeneratorWattageRating = 800f
    let gen_re = Regex::new(r"GeneratorWattageRating\s*=\s*([\d.]+)f?")?;
    if let Some(cap) = gen_re.captures(&content) {
        building.power_watts = cap[1].parse::<f64>().unwrap_or(0.0); // Positive = generation
    }

    // Extract heat output
    // Pattern: ExhaustKilowattsWhenActive = 0.5f or SelfHeatKilowattsWhenActive
    let heat_re = Regex::new(r"(?:Exhaust|SelfHeat)KilowattsWhenActive\s*=\s*([\d.]+)f?")?;
    for cap in heat_re.captures_iter(&content) {
        building.heat_dtu += cap[1].parse::<f64>().unwrap_or(0.0) * 1000.0; // kW to DTU/s
    }

    // Extract consumed elements - multiple patterns

    // Pattern 1: ConsumedElement(new Tag("Water"), 1f, true)
    let consumed_tag_re =
        Regex::new(r#"ConsumedElement\s*\(\s*new\s+Tag\s*\(\s*"(\w+)"\s*\)\s*,\s*([\d.]+)f?"#)?;
    for cap in consumed_tag_re.captures_iter(&content) {
        let element = cap[1].to_string();
        let rate = cap[2].parse::<f64>().unwrap_or(0.0);
        building.inputs.push((element, rate));
    }

    // Pattern 2: ConsumedElement(SimHashes.Water, 1f) - older format
    let consumed_hash_re =
        Regex::new(r"ConsumedElement\s*\(\s*SimHashes\.(\w+)\s*,\s*([\d.]+)f?")?;
    for cap in consumed_hash_re.captures_iter(&content) {
        let element = cap[1].to_string();
        let rate = cap[2].parse::<f64>().unwrap_or(0.0);
        if !building.inputs.iter().any(|(e, _)| e == &element) {
            building.inputs.push((element, rate));
        }
    }

    // Pattern 3: CreateSimpleFormula(SimHashes.X.CreateTag(), rate, ...) for generators
    let formula_re =
        Regex::new(r"CreateSimpleFormula\s*\(\s*SimHashes\.(\w+)\.CreateTag\(\)\s*,\s*([\d.]+)f?")?;
    for cap in formula_re.captures_iter(&content) {
        let element = cap[1].to_string();
        let rate = cap[2].parse::<f64>().unwrap_or(0.0);
        if !building.inputs.iter().any(|(e, _)| e == &element) {
            building.inputs.push((element, rate));
        }
    }

    // Extract output elements
    // Pattern: new ElementConverter.OutputElement(0.888f, SimHashes.Oxygen, ...)
    let output_re = Regex::new(r"OutputElement\s*\(\s*([\d.]+)f?\s*,\s*(?:SimHashes\.)?(\w+)")?;
    for cap in output_re.captures_iter(&content) {
        let rate = cap[1].parse::<f64>().unwrap_or(0.0);
        let element = cap[2].to_string();
        building.outputs.push((element, rate));
    }

    // ElementConsumer patterns (pumps, filters, etc.)
    // Pattern: elementConsumer.consumptionRate = 0.5f
    let consumer_rate_re = Regex::new(r"elementConsumer\.consumptionRate\s*=\s*([\d.]+)f?")?;
    if let Some(cap) = consumer_rate_re.captures(&content) {
        let rate = cap[1].parse::<f64>().unwrap_or(0.0);

        // Determine element type from Configuration or ConduitType
        let element = if content.contains("Configuration.AllGas") || content.contains("ConduitType.Gas") {
            "Gas".to_string()
        } else if content.contains("Configuration.AllLiquid") || content.contains("ConduitType.Liquid") {
            "Liquid".to_string()
        } else if let Some(elem_cap) = Regex::new(r"SimHashes\.(\w+)")?.captures(&content) {
            elem_cap[1].to_string()
        } else {
            "Unknown".to_string()
        };

        if !building.inputs.iter().any(|(e, _)| e == &element) {
            building.inputs.push((element, rate));
        }
    }

    // ConduitConsumer patterns (buildings that consume from pipes)
    // Pattern: conduitConsumer.consumptionRate = 1f
    let conduit_rate_re = Regex::new(r"conduitConsumer\.consumptionRate\s*=\s*([\d.]+)f?")?;
    if let Some(cap) = conduit_rate_re.captures(&content) {
        let rate = cap[1].parse::<f64>().unwrap_or(0.0);

        // Determine element type from capacityTag or conduitType
        let element = if let Some(tag_cap) = Regex::new(r"capacityTag\s*=\s*(?:ElementLoader\.FindElementByHash\()?SimHashes\.(\w+)")?.captures(&content) {
            tag_cap[1].to_string()
        } else if let Some(tag_cap) = Regex::new(r"capacityTag\s*=\s*GameTagExtensions\.Create\(SimHashes\.(\w+)\)")?.captures(&content) {
            tag_cap[1].to_string()
        } else if content.contains("ConduitType.Gas") {
            "Gas".to_string()
        } else if content.contains("ConduitType.Liquid") {
            "Liquid".to_string()
        } else {
            "Unknown".to_string()
        };

        if !building.inputs.iter().any(|(e, _)| e == &element) {
            building.inputs.push((element, rate));
        }
    }

    // Check for EnergyGenerator output elements (for power generators)
    // Pattern: new EnergyGenerator.OutputItem(SimHashes.CarbonDioxide, 0.02f)
    let gen_output_re =
        Regex::new(r"EnergyGenerator\.OutputItem\s*\(\s*(?:SimHashes\.)?(\w+)\s*,\s*([\d.]+)f?")?;
    for cap in gen_output_re.captures_iter(&content) {
        let element = cap[1].to_string();
        let rate = cap[2].parse::<f64>().unwrap_or(0.0);
        building.outputs.push((element, rate));
    }

    // Check for EnergyGenerator input (fuel)
    // Pattern: new EnergyGenerator.InputItem(Tag, 0.1f, 1f)
    let gen_input_re = Regex::new(
        r"EnergyGenerator\.InputItem\s*\(\s*(?:SimHashes\.)?(\w+)(?:\.CreateTag\(\))?\s*,\s*([\d.]+)f?",
    )?;
    for cap in gen_input_re.captures_iter(&content) {
        let element = cap[1].to_string();
        let rate = cap[2].parse::<f64>().unwrap_or(0.0);
        if !building.inputs.iter().any(|(e, _)| e == &element) {
            building.inputs.push((element, rate));
        }
    }

    Ok(Some(building))
}

/// Extract all building data from decompiled source and populate database
pub fn extract_to_database(conn: &Connection, decompiled_dir: &Path) -> Result<ExtractStats> {
    let mut stats = ExtractStats::default();

    println!("Scanning {} for building configs...", decompiled_dir.display());
    let config_files = find_config_files(decompiled_dir)?;
    println!("Found {} potential building config files", config_files.len());

    for filepath in &config_files {
        match parse_building_config(filepath) {
            Ok(Some(extracted)) => {
                // Create building record
                let building = Building {
                    id: extracted.id.clone(),
                    name: extracted.id.clone(), // Use ID as name for now
                    category: None,
                    power_watts: extracted.power_watts,
                    heat_output_dtu: extracted.heat_dtu,
                    construction_time_s: None,
                };

                db::upsert_building(conn, &building)?;

                // Insert inputs
                for (element, rate) in &extracted.inputs {
                    let input = BuildingInput {
                        building_id: extracted.id.clone(),
                        resource_id: element.clone(),
                        rate_kg_per_s: *rate,
                    };
                    db::insert_building_input(conn, &input)?;
                }

                // Insert outputs
                for (element, rate) in &extracted.outputs {
                    let output = BuildingOutput {
                        building_id: extracted.id.clone(),
                        resource_id: element.clone(),
                        rate_kg_per_s: *rate,
                    };
                    db::insert_building_output(conn, &output)?;
                }

                stats.buildings += 1;
                stats.inputs += extracted.inputs.len();
                stats.outputs += extracted.outputs.len();

                println!(
                    "  Parsed: {} (power: {}W, inputs: {}, outputs: {})",
                    extracted.id,
                    extracted.power_watts,
                    extracted.inputs.len(),
                    extracted.outputs.len()
                );
            }
            Ok(None) => {
                // Not a building config we can parse
                stats.skipped += 1;
            }
            Err(e) => {
                eprintln!("  Error parsing {}: {}", filepath.display(), e);
                stats.errors += 1;
            }
        }
    }

    Ok(stats)
}

#[derive(Debug, Default)]
pub struct ExtractStats {
    pub buildings: usize,
    pub inputs: usize,
    pub outputs: usize,
    pub skipped: usize,
    pub errors: usize,
}

impl std::fmt::Display for ExtractStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Extracted {} buildings ({} inputs, {} outputs). Skipped: {}, Errors: {}",
            self.buildings, self.inputs, self.outputs, self.skipped, self.errors
        )
    }
}
