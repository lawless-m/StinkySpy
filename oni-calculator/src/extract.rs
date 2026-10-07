//! C# source code extraction for ONI building data
//!
//! Parses decompiled C# source from Assembly-CSharp.dll to extract
//! building definitions, inputs, outputs, and power requirements.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use regex::Regex;
use rusqlite::Connection;
use walkdir::WalkDir;

use crate::db;
use crate::models::{
    Building, BuildingInput, BuildingOutput,
    Critter, CritterInput, CritterOutput,
    Food,
    Plant, PlantNeed, PlantOutput, PlantAtmosphere,
    RecipeInput, RecipeOutput,
};

/// Extracted building data before database insertion
#[derive(Debug, Default)]
struct ExtractedBuilding {
    id: String,
    power_watts: f64,
    heat_dtu: f64,
    inputs: Vec<(String, f64)>,  // (element, rate_kg_s)
    outputs: Vec<(String, f64)>, // (element, rate_kg_s)
}

/// Parsed recipe data before database insertion
struct ParsedRecipe {
    building_id: String,
    name: String,
    inputs: Vec<(String, f64)>,
    outputs: Vec<(String, f64)>,
}

/// Events for sequential recipe parsing
enum RecipeEvent {
    ArrayDef(String, Vec<(String, f64)>),
    RecipeUse(String, String, String), // building_id, input_var, output_var
}

// ========== Common Helpers ==========

/// Extract comma-separated arguments from inside balanced delimiters.
/// Caller should pass the content starting AFTER the opening paren/bracket.
fn extract_balanced_args(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut depth = 1;
    let mut current = String::new();
    let mut in_string = false;

    for ch in s.chars() {
        if ch == '"' {
            in_string = !in_string;
            current.push(ch);
        } else if in_string {
            current.push(ch);
        } else {
            match ch {
                '(' | '[' | '{' => { depth += 1; current.push(ch); }
                ')' | ']' | '}' => {
                    depth -= 1;
                    if depth == 0 {
                        let trimmed = current.trim().to_string();
                        if !trimmed.is_empty() { args.push(trimmed); }
                        return args;
                    }
                    current.push(ch);
                }
                ',' if depth == 1 => {
                    args.push(current.trim().to_string());
                    current.clear();
                }
                '\n' | '\r' | '\t' => current.push(' '),
                _ => current.push(ch),
            }
        }
    }
    args
}

/// Find a method call in content and extract its arguments
fn find_method_args(content: &str, method: &str) -> Option<Vec<String>> {
    let idx = content.find(method)?;
    let after = idx + method.len();
    let paren = content[after..].find('(')? + after;
    Some(extract_balanced_args(&content[paren + 1..]))
}

/// Resolve a float from a literal or simple variable assignment
fn resolve_float(content: &str, expr: &str) -> Option<f64> {
    let expr = expr.trim();
    if let Ok(val) = expr.trim_end_matches('f').parse::<f64>() {
        return Some(val);
    }
    let re = Regex::new(&format!(
        r"(?:float|double|int)\s+{}\s*=\s*([\d.eE+-]+)f?",
        regex::escape(expr)
    )).ok()?;
    re.captures(content)?.get(1)?.as_str().parse::<f64>().ok()
}

/// Resolve an amount expression (literal, variable, float array, or simple multiplication)
fn resolve_amount(content: &str, expr: &str) -> Option<f64> {
    let expr = expr.trim();
    if let Ok(val) = expr.trim_end_matches('f').parse::<f64>() {
        return Some(val);
    }
    // Float array: new float[] { 1f, 2f } - take first
    if expr.contains("new float[]") {
        if let (Some(s), Some(e)) = (expr.find('{'), expr.find('}')) {
            let first = expr[s+1..e].split(',').next()?.trim().trim_end_matches('f');
            return first.parse::<f64>().ok();
        }
    }
    // Parenthesized: (expr)
    if expr.starts_with('(') && expr.ends_with(')') {
        return resolve_amount(content, &expr[1..expr.len()-1]);
    }
    // Multiplication: a * b
    if let Some(idx) = expr.find(" * ") {
        let a = resolve_amount(content, expr[..idx].trim())?;
        let b = resolve_amount(content, expr[idx+3..].trim())?;
        return Some(a * b);
    }
    if let Some(idx) = expr.find('*') {
        let a = resolve_amount(content, expr[..idx].trim())?;
        let b = resolve_amount(content, expr[idx+1..].trim())?;
        return Some(a * b);
    }
    // Variable reference
    resolve_float(content, expr)
}

/// Resolve a tag/resource ID from various C# expression patterns
fn resolve_tag_expr(expr: &str, content: &str, config_ids: &HashMap<String, String>) -> Option<String> {
    let expr = expr.trim();

    // "StringLiteral" or "StringLiteral".ToTag()
    if let Some(cap) = Regex::new(r#"^"(\w+)"(?:\.ToTag\(\))?$"#).ok()?.captures(expr) {
        return Some(cap[1].to_string());
    }
    // SimHashes.X.CreateTag()
    if let Some(cap) = Regex::new(r"SimHashes\.(\w+)\.CreateTag\(\)").ok()?.captures(expr) {
        return Some(cap[1].to_string());
    }
    // SimHashes.X.ToString() or .ToString().ToTag()
    if let Some(cap) = Regex::new(r"SimHashes\.(\w+)\.ToString\(\)").ok()?.captures(expr) {
        return Some(cap[1].to_string());
    }
    // ElementLoader.FindElementByHash(SimHashes.X).tag
    if let Some(cap) = Regex::new(r"ElementLoader\.FindElementByHash\(SimHashes\.(\w+)\)\.tag").ok()?.captures(expr) {
        return Some(cap[1].to_string());
    }
    // XConfig.ID or XConfig.ID.ToTag()
    if let Some(cap) = Regex::new(r"(\w+Config)\.ID").ok()?.captures(expr) {
        let name = cap[1].to_string();
        return config_ids.get(&name).cloned()
            .or_else(|| Some(name.trim_end_matches("Config").to_string()));
    }
    // GameTags.X (not followed by .Append)
    if let Some(cap) = Regex::new(r"^GameTags\.(\w+)$").ok()?.captures(expr) {
        return Some(cap[1].to_string());
    }
    // GameTags.X.Append(...) - use the GameTags value
    if let Some(cap) = Regex::new(r"GameTags\.(\w+)\.Append").ok()?.captures(expr) {
        return Some(cap[1].to_string());
    }
    // new Tag[] { ... } - take first element
    if expr.contains("new Tag[]") {
        if let (Some(s), Some(e)) = (expr.find('{'), expr.find('}')) {
            let first = expr[s+1..e].split(',').next()?.trim();
            return resolve_tag_expr(first, content, config_ids);
        }
    }
    // var.tag - resolve from variable assignment
    if expr.ends_with(".tag") {
        let var = &expr[..expr.len() - 4];
        let re = Regex::new(&format!(
            r"(?:Element|Tag)\s+{}\s*=\s*(?:ElementLoader\.FindElementByHash\()?SimHashes\.(\w+)",
            regex::escape(var)
        )).ok()?;
        if let Some(cap) = re.captures(content) {
            return Some(cap[1].to_string());
        }
    }
    // Plain variable name (tag, tag2, etc.)
    if Regex::new(r"^\w+$").ok()?.is_match(expr) {
        // Tag var = SimHashes.X.CreateTag() or ElementLoader...
        let re = Regex::new(&format!(
            r"Tag\s+{}\s*=\s*(?:ElementLoader\.FindElementByHash\()?SimHashes\.(\w+)",
            regex::escape(expr)
        )).ok()?;
        if let Some(cap) = re.captures(content) {
            return Some(cap[1].to_string());
        }
        // Tag var = GameTags.X
        let re2 = Regex::new(&format!(
            r"Tag\s+{}\s*=\s*GameTags\.(\w+)",
            regex::escape(expr)
        )).ok()?;
        if let Some(cap) = re2.captures(content) {
            return Some(cap[1].to_string());
        }
    }
    None
}

/// Extract a string or Config.ID value from a crop_id expression, with variable resolution
fn resolve_crop_id(expr: &str, content: &str, config_ids: &HashMap<String, String>) -> Option<String> {
    let expr = expr.trim();
    if expr == "null" { return None; }
    // String literal: "WoodLog"
    if let Some(cap) = Regex::new(r#""(\w+)""#).ok()?.captures(expr) {
        return Some(cap[1].to_string());
    }
    // Config.ID reference: PrickleFruitConfig.ID
    if let Some(cap) = Regex::new(r"(\w+Config)\.ID").ok()?.captures(expr) {
        let name = cap[1].to_string();
        return config_ids.get(&name).cloned()
            .or_else(|| Some(name.trim_end_matches("Config").to_string()));
    }
    // SimHashes.X.ToString()
    if let Some(cap) = Regex::new(r"SimHashes\.(\w+)\.ToString\(\)").ok()?.captures(expr) {
        return Some(cap[1].to_string());
    }
    // Variable reference - resolve from content
    if Regex::new(r"^\w+$").ok()?.is_match(expr) {
        // string var = "Literal"
        let re_str = Regex::new(&format!(
            r#"(?:string|Tag)\s+{}\s*=\s*"(\w+)""#,
            regex::escape(expr)
        )).ok()?;
        if let Some(cap) = re_str.captures(content) {
            return Some(cap[1].to_string());
        }
        // string var = XConfig.ID
        let re_cfg = Regex::new(&format!(
            r"(?:string|Tag)\s+{}\s*=\s*(\w+Config)\.ID",
            regex::escape(expr)
        )).ok()?;
        if let Some(cap) = re_cfg.captures(content) {
            let name = cap[1].to_string();
            return config_ids.get(&name).cloned()
                .or_else(|| Some(name.trim_end_matches("Config").to_string()));
        }
        // string var = SimHashes.X.ToString()
        let re_sim = Regex::new(&format!(
            r"(?:string|Tag)\s+{}\s*=\s*SimHashes\.(\w+)\.ToString\(\)",
            regex::escape(expr)
        )).ok()?;
        if let Some(cap) = re_sim.captures(content) {
            return Some(cap[1].to_string());
        }
    }
    None
}

/// Parse SimHashes array like: new SimHashes[] { SimHashes.CarbonDioxide, SimHashes.Oxygen }
fn parse_simhashes_array(expr: &str) -> Vec<String> {
    if expr.trim() == "null" { return vec![]; }
    Regex::new(r"SimHashes\.(\w+)").unwrap()
        .captures_iter(expr)
        .map(|c| c[1].to_string())
        .collect()
}

/// Parse PlantElementAbsorber.ConsumeInfo entries from irrigation/fertilizer calls
fn parse_consume_infos(content: &str, plant_id: &str, method: &str) -> Vec<PlantNeed> {
    let mut needs = Vec::new();
    let Some(idx) = content.find(method) else { return needs };
    let rest = &content[idx..];
    let Some(brace_offset) = rest.find('{') else { return needs };
    let body_start = idx + brace_offset + 1;

    // Find matching closing brace
    let mut depth = 1;
    let mut body_end = body_start;
    for ch in content[body_start..].chars() {
        match ch {
            '{' => depth += 1,
            '}' => { depth -= 1; if depth == 0 { break; } }
            _ => {}
        }
        body_end += ch.len_utf8();
    }
    let body = &content[body_start..body_end];

    let tag_re = Regex::new(r"tag\s*=\s*([^,}\n]+)").unwrap();
    let rate_re = Regex::new(r"massConsumptionRate\s*=\s*([\d.]+)f?").unwrap();

    for block in body.split("new PlantElementAbsorber.ConsumeInfo") {
        let tag_match = tag_re.captures(block);
        let rate_match = rate_re.captures(block);
        if let (Some(tag_cap), Some(rate_cap)) = (tag_match, rate_match) {
            let tag_expr = tag_cap[1].trim();
            let rate = rate_cap[1].parse::<f64>().unwrap_or(0.0);
            let empty_map = HashMap::new();
            if let Some(resource_id) = resolve_tag_expr(tag_expr, content, &empty_map) {
                needs.push(PlantNeed {
                    plant_id: plant_id.to_string(),
                    resource_id,
                    rate_kg_per_s: rate,
                });
            }
        }
    }
    needs
}

// ========== Config ID Map & Crops Lookup ==========

/// Build a map from config class names (e.g. "MushroomConfig") to their ID constants
fn build_config_id_map(decompiled_dir: &Path) -> Result<HashMap<String, String>> {
    let mut map = HashMap::new();
    let id_re = Regex::new(r#"(?:public\s+)?(?:const|static)\s+string\s+ID\s*=\s*"(\w+)""#)?;

    for entry in WalkDir::new(decompiled_dir)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if filename.ends_with("Config.cs") {
            if let Ok(content) = fs::read_to_string(path) {
                if let Some(cap) = id_re.captures(&content) {
                    let class_name = filename.trim_end_matches(".cs");
                    map.insert(class_name.to_string(), cap[1].to_string());
                }
            }
        }
    }
    Ok(map)
}

/// Parse CROPS.cs to build a crop lookup: crop_id -> (duration_seconds, num_produced)
fn parse_crops_table(decompiled_dir: &Path, config_ids: &HashMap<String, String>) -> Result<HashMap<String, (f64, i32)>> {
    let mut crops = HashMap::new();
    let crops_file = decompiled_dir.join("TUNING").join("CROPS.cs");
    if !crops_file.exists() {
        return Ok(crops);
    }

    let content = fs::read_to_string(&crops_file)?;
    let re = Regex::new(
        r#"new\s+Crop\.CropVal\s*\(\s*(?:"([^"]+)"|(\w+Config)\.ID|SimHashes\.(\w+)\.ToString\(\))\s*,\s*([\d.]+)f?\s*,\s*(\d+)"#
    )?;

    for cap in re.captures_iter(&content) {
        let crop_id = if let Some(m) = cap.get(1) {
            m.as_str().to_string()
        } else if let Some(m) = cap.get(2) {
            config_ids.get(m.as_str()).cloned()
                .unwrap_or_else(|| m.as_str().trim_end_matches("Config").to_string())
        } else if let Some(m) = cap.get(3) {
            m.as_str().to_string()
        } else {
            continue;
        };
        let duration_s = cap[4].parse::<f64>().unwrap_or(600.0);
        let num = cap[5].parse::<i32>().unwrap_or(1);
        crops.insert(crop_id, (duration_s, num));
    }
    Ok(crops)
}

// ========== Building Extraction ==========

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

    // Pattern 2b: ConsumedElement(GameTagExtensions.Create(SimHashes.Water), 1f, true)
    let consumed_gametag_re =
        Regex::new(r"ConsumedElement\s*\(\s*GameTagExtensions\.Create\(SimHashes\.(\w+)\)\s*,\s*([\d.]+)f?")?;
    for cap in consumed_gametag_re.captures_iter(&content) {
        let element = cap[1].to_string();
        let rate = cap[2].parse::<f64>().unwrap_or(0.0);
        if !building.inputs.iter().any(|(e, _)| e == &element) {
            building.inputs.push((element, rate));
        }
    }

    // Pattern 3: CreateSimpleFormula(input, inputRate, capacity, output, outputRate, ...) for generators
    // Example: CreateSimpleFormula(SimHashes.Carbon.CreateTag(), 1f, 600f, SimHashes.CarbonDioxide, 0.02f, ...)
    let formula_re = Regex::new(
        r"CreateSimpleFormula\s*\(\s*SimHashes\.(\w+)\.CreateTag\(\)\s*,\s*([\d.]+)f?\s*,\s*[\d.]+f?\s*,\s*SimHashes\.(\w+)\s*,\s*([\d.]+)f?"
    )?;
    for cap in formula_re.captures_iter(&content) {
        let in_element = cap[1].to_string();
        let in_rate = cap[2].parse::<f64>().unwrap_or(0.0);
        let out_element = cap[3].to_string();
        let out_rate = cap[4].parse::<f64>().unwrap_or(0.0);

        if !building.inputs.iter().any(|(e, _)| e == &in_element) {
            building.inputs.push((in_element, in_rate));
        }
        if out_element != "Void" && out_rate > 0.0 {
            building.outputs.push((out_element, out_rate));
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

// ========== Critter Extraction ==========

/// Find all critter config files (excluding babies and base classes)
fn find_critter_files(decompiled_dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    let mut configs = Vec::new();

    for entry in WalkDir::new(decompiled_dir)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.extension().map_or(false, |ext| ext == "cs") {
            let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            // Match critter configs but exclude Baby* and Base* configs
            if filename.ends_with("Config.cs") && !filename.starts_with("Baby") && !filename.starts_with("Base") {
                let content = fs::read_to_string(path).unwrap_or_default();
                // Include files that have IEntityConfig and critter-related content
                // Must have either feeding patterns or be a fertile creature
                if content.contains("IEntityConfig") && content.contains("public const string ID") &&
                   (content.contains("KG") && content.contains("CYCLE") || // Feeding pattern
                    content.contains("DAYS_PLANT") || // Plant eating
                    content.contains("Diet(") ||  // Diet definition
                    content.contains("CALORIES_PER") || // Calorie constants
                    content.contains("ExtendEntityToFertileCreature")) { // Fertile creatures
                    configs.push(path.to_path_buf());
                }
            }
        }
    }

    Ok(configs)
}

/// Parse a single critter config file
fn parse_critter_config(filepath: &Path) -> Result<Option<(Critter, Vec<CritterInput>, Vec<CritterOutput>)>> {
    let content = fs::read_to_string(filepath)
        .with_context(|| format!("Failed to read {}", filepath.display()))?;

    // Extract critter ID
    let const_id_re = Regex::new(r#"(?:public\s+)?const\s+string\s+ID\s*=\s*"(\w+)""#)?;
    let id = if let Some(cap) = const_id_re.captures(&content) {
        cap[1].to_string()
    } else {
        return Ok(None);
    };

    // Skip non-critter entities (shells, poops, crowns, babies, fillets, meat)
    if id.ends_with("Shell") || id.ends_with("Poop") || id.ends_with("Crown") ||
       id.ends_with("Baby") || id.ends_with("Fillet") || id.ends_with("Meat") {
        return Ok(None);
    }

    // Check if there's a corresponding Base config file
    // First, try to detect if the config calls a Base*Config (e.g., BasePacuConfig.CreatePrefab)
    let base_call_re = Regex::new(r"(Base\w+Config)\.")?;
    let base_content = if let Some(cap) = base_call_re.captures(&content) {
        // Found a call to a Base config, use that file
        let base_config_filename = format!("{}.cs", &cap[1]);
        let base_config_path = filepath.with_file_name(base_config_filename);
        if base_config_path.exists() {
            fs::read_to_string(&base_config_path).unwrap_or_default()
        } else {
            String::new()
        }
    } else {
        // No Base config call found, try the naming convention (e.g., BasePacuConfig for PacuConfig)
        let base_config_filename = format!("Base{}", filepath.file_name().unwrap().to_str().unwrap());
        let base_config_path = filepath.with_file_name(base_config_filename);
        if base_config_path.exists() {
            fs::read_to_string(&base_config_path).unwrap_or_default()
        } else {
            String::new()
        }
    };

    // Extract food consumption - try multiple patterns from both configs
    let kg_eaten_re = Regex::new(r"KG_(?:ORE_|KELP_)?EATEN_PER_CYCLE\s*=\s*([\d.]+)f?")?;
    let days_plant_re = Regex::new(r"DAYS_PLANT_GROWTH_EATEN_PER_CYCLE\s*=\s*([\d.]+)f?")?;
    let calories_per_kg_re = Regex::new(r"CALORIES_PER_KG_OF_(?:ORE|DIRT|PLANT_EATEN|KELP)\s*=\s*([\d.]+)f?")?;

    // Track which pattern was matched so we know if we need to recalculate
    let mut is_calories_per_kg_pattern = false;

    // Try to extract from main config first, then fall back to base config
    let combined_content = format!("{}\n{}", content, base_content);

    let kg_per_cycle = if let Some(cap) = kg_eaten_re.captures(&combined_content) {
        // Pattern 1: Direct kg/cycle (Hatch, Puft, Crab, Pacu etc.)
        cap[1].parse::<f64>().unwrap_or(0.0)
    } else if let Some(cap) = days_plant_re.captures(&combined_content) {
        // Pattern 2: Plant growth days/cycle (Drecko, Pip/Squirrel)
        // Approximate as plant growth units (not exact kg, but useful for comparison)
        cap[1].parse::<f64>().unwrap_or(0.0) * 100.0  // Scale for visibility
    } else if let Some(cap) = calories_per_kg_re.captures(&combined_content) {
        // Pattern 3: Has CALORIES_PER_KG - try to calculate consumption
        // If we find calories_per_cycle later, we'll calculate: calories_per_cycle / calories_per_kg
        let calories_per_kg = cap[1].parse::<f64>().unwrap_or(1000.0);
        is_calories_per_kg_pattern = true;
        // We'll update this after we get calories_per_cycle from tuning file
        // For now, use a placeholder that we'll recalculate
        calories_per_kg  // Store as placeholder
    } else {
        // Pattern 4: No consumption pattern found - might be hunter/grazer
        // Don't skip, just set to 0 and continue
        0.0
    };

    // Extract min poop size
    let poop_re = Regex::new(r"MIN_POOP_SIZE_IN_KG\s*=\s*([\d.]+)f?")?;
    let min_poop_kg = if let Some(cap) = poop_re.captures(&content) {
        cap[1].parse::<f64>().unwrap_or(25.0)
    } else {
        25.0
    };

    // Extract conversion efficiency from diet function call
    let efficiency_re = Regex::new(r"CONVERSION_EFFICIENCY\.(\w+)")?;
    let conversion_efficiency = if let Some(cap) = efficiency_re.captures(&content) {
        match cap[1].as_ref() {
            "BAD_2" => 0.1,
            "BAD_1" => 0.25,
            "NORMAL" => 0.5,
            "GOOD_1" => 0.75,
            "GOOD_2" => 0.95,
            "GOOD_3" => 1.0,
            _ => 0.5,
        }
    } else {
        0.5
    };

    // Extract output element (what they poop) - check both configs
    // Try pattern 0: Tag tag = SimHashes.Element.CreateTag(); (variable assignment in Base configs)
    let tag_var_re = Regex::new(r"Tag\s+tag\s*=\s*SimHashes\.(\w+)\.CreateTag\(\)")?;
    let output_element = if let Some(cap) = tag_var_re.captures(&combined_content) {
        cap[1].to_string()
    } else if let Some(cap) = Regex::new(r"(?:public\s+)?static\s+Tag\s+POOP_ELEMENT\s*=\s*SimHashes\.(\w+)\.CreateTag\(\)")?.captures(&combined_content) {
        // Try pattern 1: POOP_ELEMENT constant
        cap[1].to_string()
    } else {
        // Try pattern 2: Diet functions like BasicDiet, BasicRockDiet, etc - first parameter is poop tag
        let diet_re = Regex::new(r"(?:BasicDiet|BasicRockDiet|HardRockDiet|MetalDiet|VeggieDiet|FoodDiet|SimpleOreDiet)\s*\(\s*(?:SimHashes\.)?(\w+)(?:\.CreateTag\(\))?")?;
        if let Some(cap) = diet_re.captures(&combined_content) {
            cap[1].to_string()
        } else {
            // Try pattern 3: SetupDiet(gameObject, inputTag, outputTag, ...) - third parameter is poop tag
            let setup_re = Regex::new(r"SetupDiet\s*\([^,]+,\s*[^,]+,\s*SimHashes\.(\w+)\.CreateTag\(\)")?;
            if let Some(cap) = setup_re.captures(&combined_content) {
                cap[1].to_string()
            } else {
                // Try pattern 4: new Diet.Info with second parameter as poop tag
                let diet_info_re = Regex::new(r"new\s+Diet\.Info\s*\([^,]+,\s*(?:SimHashes\.)?(\w+)(?:\.CreateTag\(\))?")?;
                if let Some(cap) = diet_info_re.captures(&combined_content) {
                    cap[1].to_string()
                } else {
                    "Unknown".to_string()
                }
            }
        }
    };

    // Try to find tuning file for calories, egg mass, and pen size
    let tuning_filename = filepath.file_name().unwrap().to_str().unwrap().replace("Config.cs", "Tuning.cs");
    let tuning_path = filepath.with_file_name(tuning_filename);
    let (calories_per_cycle, egg_mass_kg, pen_size_tiles) = if tuning_path.exists() {
        let tuning_content = fs::read_to_string(&tuning_path).unwrap_or_default();
        let cal_re = Regex::new(r"STANDARD_CALORIES_PER_CYCLE\s*=\s*([\d.]+)f?")?;
        let egg_re = Regex::new(r"EGG_MASS\s*=\s*([\d.]+)f?")?;
        let pen_re = Regex::new(r"PEN_SIZE_PER_CREATURE\s*=\s*(?:CREATURES\.SPACE_REQUIREMENTS\.)?(\w+)(?:\s*/\s*(\d+))?")?;

        let calories = if let Some(cap) = cal_re.captures(&tuning_content) {
            cap[1].parse::<f64>().unwrap_or(700000.0)
        } else {
            700000.0
        };
        let egg = if let Some(cap) = egg_re.captures(&tuning_content) {
            cap[1].parse::<f64>().unwrap_or(2.0)
        } else {
            2.0
        };
        let pen_size = if let Some(cap) = pen_re.captures(&tuning_content) {
            // Map TIER names to actual tile values
            let base_size = match cap[1].as_ref() {
                "TIER1" => 4,
                "TIER2" => 8,
                "TIER3" => 12,
                "TIER4" => 16,
                "TIER5" => 20,
                num => num.parse::<i32>().unwrap_or(12), // Direct number or default
            };
            // Check if there's a division operation (e.g., TIER3 / 2)
            if let Some(divisor) = cap.get(2) {
                base_size / divisor.as_str().parse::<i32>().unwrap_or(1)
            } else {
                base_size
            }
        } else {
            12  // Default TIER3
        };
        (calories, egg, pen_size)
    } else {
        (700000.0, 2.0, 12)  // Defaults
    };

    // Determine species from ID
    let species = if id.contains("Hatch") {
        "Hatch"
    } else if id.contains("Pacu") {
        "Pacu"
    } else if id.contains("Drecko") {
        "Drecko"
    } else if id.contains("Puft") {
        "Puft"
    } else if id.contains("LightBug") {
        "LightBug"
    } else if id.contains("Mole") {
        "Mole"
    } else if id.contains("Squirrel") || id.contains("Pip") {
        "Pip"
    } else if id.contains("Moo") {
        "Moo"
    } else if id.contains("Stego") {
        "Stego"
    } else if id.contains("Deer") {
        "Deer"
    } else if id.contains("Belly") {
        "Belly"
    } else if id.contains("Crab") {
        "Crab"
    } else if id.contains("Divergent") {
        "Divergent"
    } else if id.contains("Oilfloater") || id.contains("OilFloater") {
        "Oilfloater"
    } else if id.contains("Staterpillar") {
        "Staterpillar"
    } else if id.contains("Raptor") {
        "Raptor"
    } else if id.contains("Seal") {
        "Seal"
    } else if id.contains("Chameleon") {
        "Chameleon"
    } else {
        "Unknown"
    }.to_string();

    // Recalculate kg_per_cycle if it was extracted from CALORIES_PER_KG pattern
    // Only recalculate if we actually matched the calories_per_kg_re pattern
    let kg_per_cycle = if is_calories_per_kg_pattern && kg_per_cycle > 0.0 && calories_per_cycle > 0.0 {
        // This is CALORIES_PER_KG, calculate actual consumption
        calories_per_cycle / kg_per_cycle
    } else {
        kg_per_cycle
    };

    let critter = Critter {
        id: id.clone(),
        name: id.clone(),
        species,
        calories_per_cycle,
        kg_per_cycle,
        conversion_efficiency,
        min_poop_kg,
        egg_mass_kg,
        pen_size_tiles,
    };

    let outputs = vec![CritterOutput {
        critter_id: id.clone(),
        resource_id: output_element,
        rate_kg_per_cycle: kg_per_cycle * conversion_efficiency,
    }];

    // Extract diet types from function calls (check combined_content to include Base configs)
    let mut inputs = Vec::new();

    // Check for BasicRockDiet
    if combined_content.contains("BasicRockDiet") {
        for element in &["Sand", "SandStone", "Clay", "CrushedRock", "Dirt", "SedimentaryRock", "Shale"] {
            inputs.push(CritterInput {
                critter_id: id.clone(),
                resource_id: element.to_string(),
                food_type: "EatSolid".to_string(),
            });
        }
    }

    // Check for HardRockDiet
    if combined_content.contains("HardRockDiet") {
        for element in &["SedimentaryRock", "IgneousRock", "Obsidian", "Granite"] {
            inputs.push(CritterInput {
                critter_id: id.clone(),
                resource_id: element.to_string(),
                food_type: "EatSolid".to_string(),
            });
        }
    }

    // Check for VeggieDiet
    if combined_content.contains("VeggieDiet") {
        for element in &["Dirt", "SlimeMold", "Algae", "Fertilizer", "ToxicSand"] {
            inputs.push(CritterInput {
                critter_id: id.clone(),
                resource_id: element.to_string(),
                food_type: "EatSolid".to_string(),
            });
        }
    }

    // Check for MetalDiet
    if combined_content.contains("MetalDiet") {
        // Metal hatches eat various metal ores
        for element in &["IronOre", "CopperOre", "AluminumOre", "GoldAmalgam", "Wolframite"] {
            inputs.push(CritterInput {
                critter_id: id.clone(),
                resource_id: element.to_string(),
                food_type: "EatSolid".to_string(),
            });
        }
    }

    // Check for FoodDiet - this one accepts ALL prepared food
    // We'll add a special marker that can be expanded later from the foods table
    if combined_content.contains("FoodDiet") {
        inputs.push(CritterInput {
            critter_id: id.clone(),
            resource_id: "ALL_FOODS".to_string(),
            food_type: "EatSolid".to_string(),
        });
    }

    // Extract inline diet definitions (for Mole, Pacu, etc.)
    // Look for SimHashes.Element.CreateTag() patterns in combined_content (includes Base configs)
    let inline_diet_re = Regex::new(r"SimHashes\.(\w+)\.CreateTag\(\)")?;
    let mut found_elements = std::collections::HashSet::new();
    let output_element_clone = outputs[0].resource_id.clone();
    for cap in inline_diet_re.captures_iter(&combined_content) {
        let element = cap[1].to_string();
        // Skip common non-food elements and output elements
        if element != output_element_clone && !["Creature", "Vacuum", "Unobtanium"].contains(&element.as_str()) {
            found_elements.insert(element);
        }
    }

    // Add unique elements found (these are likely food inputs)
    for element in found_elements {
        if !inputs.iter().any(|i| i.resource_id == element) {
            inputs.push(CritterInput {
                critter_id: id.clone(),
                resource_id: element,
                food_type: "EatSolid".to_string(),
            });
        }
    }

    Ok(Some((critter, inputs, outputs)))
}

// ========== Recipe Extraction ==========

/// Find all recipe-based building config files
fn find_recipe_files(decompiled_dir: &Path) -> Result<Vec<std::path::PathBuf>> {
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
                if content.contains("ComplexRecipe") || content.contains("ComplexRecipeManager") {
                    configs.push(path.to_path_buf());
                }
            }
        }
    }

    Ok(configs)
}

/// Parse RecipeElement entries from an array body
fn parse_recipe_elements(body: &str, full_content: &str, config_ids: &HashMap<String, String>) -> Vec<(String, f64)> {
    let mut elements = Vec::new();
    let needle = "new ComplexRecipe.RecipeElement(";

    let mut search_from = 0;
    while let Some(offset) = body[search_from..].find(needle) {
        let start = search_from + offset + needle.len();
        let args = extract_balanced_args(&body[start..]);

        if args.len() >= 2 {
            if let Some(tag) = resolve_tag_expr(&args[0], full_content, config_ids) {
                if let Some(amount) = resolve_amount(full_content, &args[1]) {
                    elements.push((tag, amount));
                }
            }
        }

        search_from = start;
    }
    elements
}

/// Parse recipe config file - extracts all ComplexRecipe definitions
fn parse_recipe_config(filepath: &Path, config_ids: &HashMap<String, String>) -> Result<Vec<ParsedRecipe>> {
    let content = fs::read_to_string(filepath)
        .with_context(|| format!("Failed to read {}", filepath.display()))?;

    let mut recipes = Vec::new();
    let mut events: Vec<(usize, RecipeEvent)> = Vec::new();

    // Find array definitions: ComplexRecipe.RecipeElement[] varName = new ...[]
    let array_re = Regex::new(
        r"ComplexRecipe\.RecipeElement\[\]\s+(\w+)\s*=\s*new\s+ComplexRecipe\.RecipeElement\[\]"
    )?;
    for cap in array_re.captures_iter(&content) {
        let pos = cap.get(0).unwrap().start();
        let var_name = cap[1].to_string();
        let match_end = cap.get(0).unwrap().end();

        // Find array body { ... }
        let rest = &content[match_end..];
        if let Some(brace_offset) = rest.find('{') {
            let body_start = match_end + brace_offset + 1;
            let mut depth = 1;
            let mut body_end = body_start;
            for ch in content[body_start..].chars() {
                match ch {
                    '{' => depth += 1,
                    '}' => { depth -= 1; if depth == 0 { break; } }
                    _ => {}
                }
                body_end += ch.len_utf8();
            }
            let body = &content[body_start..body_end];
            let elements = parse_recipe_elements(body, &content, config_ids);
            events.push((pos, RecipeEvent::ArrayDef(var_name, elements)));
        }
    }

    // Find MakeRecipeID("BuildingId", inputVar, outputVar) calls
    let recipe_re = Regex::new(
        r#"ComplexRecipeManager\.MakeRecipeID\s*\(\s*"(\w+)"\s*,\s*(\w+)\s*,\s*(\w+)\s*\)"#
    )?;
    for cap in recipe_re.captures_iter(&content) {
        let pos = cap.get(0).unwrap().start();
        events.push((pos, RecipeEvent::RecipeUse(
            cap[1].to_string(),
            cap[2].to_string(),
            cap[3].to_string(),
        )));
    }

    // Sort by file position and process sequentially
    events.sort_by_key(|(pos, _)| *pos);
    let mut arrays: HashMap<String, Vec<(String, f64)>> = HashMap::new();

    for (_, event) in events {
        match event {
            RecipeEvent::ArrayDef(name, elements) => {
                arrays.insert(name, elements);
            }
            RecipeEvent::RecipeUse(building_id, input_var, output_var) => {
                let inputs = arrays.get(&input_var).cloned().unwrap_or_default();
                let outputs = arrays.get(&output_var).cloned().unwrap_or_default();

                if inputs.is_empty() && outputs.is_empty() {
                    continue;
                }

                let name = outputs.first()
                    .map(|(id, _)| id.clone())
                    .unwrap_or_else(|| format!("{}_recipe", building_id));

                recipes.push(ParsedRecipe { building_id, name, inputs, outputs });
            }
        }
    }

    Ok(recipes)
}

// ========== Plant Extraction ==========

/// Find all plant config files
fn find_plant_files(decompiled_dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    let mut configs = Vec::new();

    for entry in WalkDir::new(decompiled_dir)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.extension().map_or(false, |ext| ext == "cs") {
            let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if filename.ends_with("PlantConfig.cs") || filename.ends_with("Config.cs") {
                let content = fs::read_to_string(path).unwrap_or_default();
                if content.contains("ExtendEntityToBasicPlant") {
                    configs.push(path.to_path_buf());
                }
            }
        }
    }

    Ok(configs)
}

/// Parse a single plant config file
fn parse_plant_config(filepath: &Path, crops: &HashMap<String, (f64, i32)>, config_ids: &HashMap<String, String>) -> Result<Option<(Plant, Vec<PlantNeed>, Vec<PlantNeed>, Vec<PlantOutput>, Vec<PlantAtmosphere>)>> {
    let content = fs::read_to_string(filepath)
        .with_context(|| format!("Failed to read {}", filepath.display()))?;

    // Extract plant ID
    let const_id_re = Regex::new(r#"(?:public\s+)?(?:const|static)\s+string\s+ID\s*=\s*"(\w+)""#)?;
    let id = if let Some(cap) = const_id_re.captures(&content) {
        cap[1].to_string()
    } else {
        return Ok(None);
    };

    // Parse ExtendEntityToBasicPlant arguments
    let args = match find_method_args(&content, "ExtendEntityToBasicPlant") {
        Some(a) if a.len() >= 10 => a,
        _ => return Ok(None),
    };

    // args[1..5]: temperatures in Kelvin -> convert to Celsius
    let temp_ll = resolve_float(&content, &args[1]).unwrap_or(0.0) - 273.15;
    let temp_wl = resolve_float(&content, &args[2]).unwrap_or(0.0) - 273.15;
    let temp_wh = resolve_float(&content, &args[3]).unwrap_or(0.0) - 273.15;
    let temp_lh = resolve_float(&content, &args[4]).unwrap_or(0.0) - 273.15;

    // args[5]: safe_elements (atmosphere)
    let atmospheres: Vec<PlantAtmosphere> = parse_simhashes_array(&args[5])
        .into_iter()
        .map(|elem| PlantAtmosphere { plant_id: id.clone(), resource_id: elem })
        .collect();

    // args[9]: crop_id (can be "string", Config.ID, variable, or null)
    let mut crop_id_str = if args.len() > 9 { resolve_crop_id(&args[9], &content, config_ids) } else { None };

    // Lookup growth duration and num_produced from CROPS.cs
    let (mut growth_s, mut num_produced) = if let Some(cid) = &crop_id_str {
        crops.get(cid.as_str()).copied().unwrap_or((600.0, 1))
    } else {
        (600.0, 0)
    };

    // Fallback: check for inline Crop.CropVal (e.g. SpaceTreeBranch)
    if crop_id_str.is_none() {
        let inline_re = Regex::new(r#"new\s+Crop\.CropVal\s*\(\s*"(\w+)"\s*,\s*([\d.]+)f?\s*,\s*(\d+)"#)?;
        if let Some(cap) = inline_re.captures(&content) {
            crop_id_str = Some(cap[1].to_string());
            growth_s = cap[2].parse::<f64>().unwrap_or(600.0);
            num_produced = cap[3].parse::<i32>().unwrap_or(1);
        }
    }

    let growth_cycles = growth_s / 600.0;

    // Build plant outputs
    let outputs: Vec<PlantOutput> = if let Some(cid) = &crop_id_str {
        vec![PlantOutput {
            plant_id: id.clone(),
            crop_id: cid.clone(),
            num_produced,
        }]
    } else {
        vec![]
    };

    // Parse irrigation and fertilizer
    let irrigation = parse_consume_infos(&content, &id, "ExtendPlantToIrrigated");
    let fertilizer = parse_consume_infos(&content, &id, "ExtendPlantToFertilizable");

    let plant = Plant {
        id: id.clone(),
        name: id.clone(),
        growth_duration_cycles: growth_cycles,
        temp_lethal_low: temp_ll,
        temp_warning_low: temp_wl,
        temp_warning_high: temp_wh,
        temp_lethal_high: temp_lh,
    };

    Ok(Some((plant, irrigation, fertilizer, outputs, atmospheres)))
}

// ========== Food Extraction ==========

/// Extract all food items from FOOD.cs
fn extract_foods(decompiled_dir: &Path) -> Result<Vec<Food>> {
    let food_file = decompiled_dir.join("TUNING").join("FOOD.cs");
    if !food_file.exists() {
        return Ok(vec![]);
    }

    let content = fs::read_to_string(&food_file)?;
    let mut foods = Vec::new();

    // Pattern: new EdiblesManager.FoodInfo("MushBar", 800000f, -1, 255.15f, 277.15f, 4800f, true, null, null);
    // Parameters: (id, calories, quality, preserve_temp, spoil_temp, spoil_time, ...)
    let food_re = Regex::new(
        r#"new\s+EdiblesManager\.FoodInfo\s*\(\s*"(\w+)"\s*,\s*([\d.]+)f?\s*,\s*(-?\d+)\s*,\s*([\d.]+)f?\s*,\s*([\d.]+)f?\s*,\s*([\d.]+)f?"#
    )?;

    for cap in food_re.captures_iter(&content) {
        let id = cap[1].to_string();
        let calories = cap[2].parse::<f64>().unwrap_or(0.0);
        let quality = cap[3].parse::<i32>().unwrap_or(0);
        let preserve_temp_k = cap[4].parse::<f64>().unwrap_or(255.15);
        let spoil_temp_k = cap[5].parse::<f64>().unwrap_or(277.15);
        let spoil_time = cap[6].parse::<f64>().unwrap_or(4800.0);

        // Convert temps from Kelvin to Celsius
        let preserve_temp_c = preserve_temp_k - 273.15;
        let spoil_temp_c = spoil_temp_k - 273.15;

        foods.push(Food {
            id: id.clone(),
            name: id.clone(),  // Use ID as name for now
            calories,
            quality,
            preserve_temp_c,
            spoil_temp_c,
            spoil_time_s: spoil_time,
        });
    }

    Ok(foods)
}

// ========== Resource Collection ==========

/// Collect all referenced resource IDs and insert stub entries into the resources table
fn collect_resources(conn: &Connection) -> Result<usize> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT resource_id FROM (
            SELECT resource_id FROM building_inputs
            UNION SELECT resource_id FROM building_outputs
            UNION SELECT resource_id FROM critter_inputs
            UNION SELECT resource_id FROM critter_outputs
            UNION SELECT resource_id FROM plant_irrigation
            UNION SELECT resource_id FROM plant_fertilizer
            UNION SELECT crop_id AS resource_id FROM plant_outputs
            UNION SELECT resource_id FROM plant_atmosphere
        ) ORDER BY resource_id"
    )?;

    let ids: Vec<String> = stmt.query_map([], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    let mut count = 0;
    for id in &ids {
        let state = guess_element_state(id);
        conn.execute(
            "INSERT OR IGNORE INTO resources (id, name, state) VALUES (?1, ?2, ?3)",
            (id, id, &state),
        )?;
        count += 1;
    }
    Ok(count)
}

fn guess_element_state(id: &str) -> Option<String> {
    match id {
        "Oxygen" | "CarbonDioxide" | "Hydrogen" | "ChlorineGas" | "ContaminatedOxygen"
        | "Methane" | "Steam" | "NaturalGas" | "Helium" => Some("Gas".to_string()),
        "Water" | "DirtyWater" | "SaltWater" | "Brine" | "Ethanol" | "CrudeOil"
        | "Petroleum" | "LiquidOxygen" | "Milk" | "NaturalResin" => Some("Liquid".to_string()),
        "Sand" | "SandStone" | "Dirt" | "Clay" | "Algae" | "Coal" | "Carbon"
        | "IronOre" | "CopperOre" | "GoldAmalgam" | "Wolframite" | "Salt"
        | "Obsidian" | "Granite" | "IgneousRock" | "SedimentaryRock" | "Shale"
        | "Regolith" | "SlimeMold" | "Rust" | "BleachStone" | "Phosphorite"
        | "Fertilizer" | "Ice" | "Glass" | "Diamond" | "Katairite" | "Gold"
        | "Tungsten" | "OxyRock" | "WoodLog" | "Sulfur" | "ToxicSand" | "Peat"
        | "Sucrose" | "Polypropylene" | "CrushedRock" | "Phosphorus" | "MilkFat"
        | "Mud" => Some("Solid".to_string()),
        _ => None,
    }
}

/// Extract all building data from decompiled source and populate database
pub fn extract_to_database(conn: &Connection, decompiled_dir: &Path) -> Result<ExtractStats> {
    let mut stats = ExtractStats::default();

    // Build lookup tables
    println!("Building config ID map...");
    let config_ids = build_config_id_map(decompiled_dir)?;
    println!("Found {} config IDs", config_ids.len());

    println!("Parsing CROPS.cs...");
    let crops = parse_crops_table(decompiled_dir, &config_ids)?;
    println!("Found {} crop types", crops.len());

    // Extract buildings
    println!("\nScanning {} for building configs...", decompiled_dir.display());
    let config_files = find_config_files(decompiled_dir)?;
    println!("Found {} potential building config files", config_files.len());

    for filepath in &config_files {
        match parse_building_config(filepath) {
            Ok(Some(extracted)) => {
                let building = Building {
                    id: extracted.id.clone(),
                    name: extracted.id.clone(),
                    category: None,
                    power_watts: extracted.power_watts,
                    heat_output_dtu: extracted.heat_dtu,
                    construction_time_s: None,
                };

                db::upsert_building(conn, &building)?;

                for (element, rate) in &extracted.inputs {
                    db::insert_building_input(conn, &BuildingInput {
                        building_id: extracted.id.clone(),
                        resource_id: element.clone(),
                        rate_kg_per_s: *rate,
                    })?;
                }

                for (element, rate) in &extracted.outputs {
                    db::insert_building_output(conn, &BuildingOutput {
                        building_id: extracted.id.clone(),
                        resource_id: element.clone(),
                        rate_kg_per_s: *rate,
                    })?;
                }

                stats.buildings += 1;
                stats.inputs += extracted.inputs.len();
                stats.outputs += extracted.outputs.len();

                println!(
                    "  Parsed: {} (power: {}W, inputs: {}, outputs: {})",
                    extracted.id, extracted.power_watts, extracted.inputs.len(), extracted.outputs.len()
                );
            }
            Ok(None) => { stats.skipped += 1; }
            Err(e) => {
                eprintln!("  Error parsing {}: {}", filepath.display(), e);
                stats.errors += 1;
            }
        }
    }

    // Extract critters
    println!("\nScanning for critter configs...");
    let critter_files = find_critter_files(decompiled_dir)?;
    println!("Found {} potential critter config files", critter_files.len());

    for filepath in &critter_files {
        match parse_critter_config(filepath) {
            Ok(Some((critter, inputs, outputs))) => {
                db::upsert_critter(conn, &critter)?;
                for input in &inputs { db::insert_critter_input(conn, input)?; }
                for output in &outputs { db::insert_critter_output(conn, output)?; }
                stats.critters += 1;
                println!("  Parsed critter: {} (species: {}, {}kg/cycle)",
                    critter.id, critter.species, critter.kg_per_cycle);
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("  Error parsing critter {}: {}", filepath.display(), e);
                stats.errors += 1;
            }
        }
    }

    // Extract recipes
    println!("\nScanning for recipe configs...");
    let recipe_files = find_recipe_files(decompiled_dir)?;
    println!("Found {} potential recipe config files", recipe_files.len());

    for filepath in &recipe_files {
        match parse_recipe_config(filepath, &config_ids) {
            Ok(parsed_recipes) => {
                for parsed in parsed_recipes {
                    let recipe_id = db::insert_recipe(conn, &parsed.building_id, &parsed.name)?;

                    for (resource_id, amount) in &parsed.inputs {
                        db::insert_recipe_input(conn, &RecipeInput {
                            recipe_id,
                            resource_id: resource_id.clone(),
                            rate_kg_per_s: *amount,
                        })?;
                    }

                    for (resource_id, amount) in &parsed.outputs {
                        db::insert_recipe_output(conn, &RecipeOutput {
                            recipe_id,
                            resource_id: resource_id.clone(),
                            rate_kg_per_s: *amount,
                        })?;
                    }

                    stats.recipes += 1;
                    println!("  Parsed recipe: {} for {}", parsed.name, parsed.building_id);
                }
            }
            Err(e) => {
                eprintln!("  Error parsing recipes {}: {}", filepath.display(), e);
                stats.errors += 1;
            }
        }
    }

    // Extract plants
    println!("\nScanning for plant configs...");
    let plant_files = find_plant_files(decompiled_dir)?;
    println!("Found {} potential plant config files", plant_files.len());

    for filepath in &plant_files {
        match parse_plant_config(filepath, &crops, &config_ids) {
            Ok(Some((plant, irrigation, fertilizer, outputs, atmosphere))) => {
                db::upsert_plant(conn, &plant)?;
                for need in &irrigation { db::insert_plant_irrigation(conn, need)?; }
                for need in &fertilizer { db::insert_plant_fertilizer(conn, need)?; }
                for output in &outputs { db::insert_plant_output(conn, output)?; }
                for atmo in &atmosphere { db::insert_plant_atmosphere(conn, atmo)?; }
                stats.plants += 1;
                println!("  Parsed plant: {} (growth: {:.1} cycles)", plant.id, plant.growth_duration_cycles);
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("  Error parsing plant {}: {}", filepath.display(), e);
                stats.errors += 1;
            }
        }
    }

    // Extract foods
    println!("\nExtracting food items from TUNING/FOOD.cs...");
    match extract_foods(decompiled_dir) {
        Ok(foods) => {
            for food in &foods { db::upsert_food(conn, food)?; }
            stats.foods = foods.len();
            println!("  Extracted {} food items", foods.len());
        }
        Err(e) => {
            eprintln!("  Error extracting foods: {}", e);
            stats.errors += 1;
        }
    }

    // Collect resource entries from all referenced IDs
    println!("\nCollecting resource entries...");
    match collect_resources(conn) {
        Ok(count) => {
            stats.resources = count;
            println!("  Collected {} resource entries", count);
        }
        Err(e) => {
            eprintln!("  Error collecting resources: {}", e);
            stats.errors += 1;
        }
    }

    Ok(stats)
}

#[derive(Debug, Default)]
pub struct ExtractStats {
    pub buildings: usize,
    pub inputs: usize,
    pub outputs: usize,
    pub critters: usize,
    pub recipes: usize,
    pub plants: usize,
    pub foods: usize,
    pub resources: usize,
    pub skipped: usize,
    pub errors: usize,
}

impl std::fmt::Display for ExtractStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Extracted {} buildings ({} inputs, {} outputs), {} critters, {} recipes, {} plants, {} foods, {} resources. Skipped: {}, Errors: {}",
            self.buildings, self.inputs, self.outputs, self.critters, self.recipes, self.plants, self.foods, self.resources, self.skipped, self.errors
        )
    }
}
