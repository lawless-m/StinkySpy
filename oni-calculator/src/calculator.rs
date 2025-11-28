//! Production chain calculator logic

use anyhow::{anyhow, Result};
use rusqlite::Connection;

use crate::db;
use crate::models::{InputRequirement, ProductionNode};

/// Calculate the production chain for a target resource at a given rate
///
/// Returns a tree of buildings needed to produce the target resource,
/// including all upstream dependencies.
pub fn calculate_production_chain(
    conn: &Connection,
    target_resource: &str,
    target_rate_kg_s: f64,
) -> Result<ProductionNode> {
    calculate_chain_recursive(conn, target_resource, target_rate_kg_s, 0)
}

fn calculate_chain_recursive(
    conn: &Connection,
    resource: &str,
    rate: f64,
    depth: usize,
) -> Result<ProductionNode> {
    const MAX_DEPTH: usize = 20; // Prevent infinite recursion

    if depth > MAX_DEPTH {
        return Err(anyhow!(
            "Maximum recursion depth exceeded - possible cycle in production chain"
        ));
    }

    // Find buildings that produce this resource
    let producers = db::get_producers(conn, resource)?;

    if producers.is_empty() {
        // This is a raw resource (no building produces it)
        return Ok(ProductionNode {
            building_id: "RAW_RESOURCE".to_string(),
            building_name: format!("{} (raw input)", resource),
            count: 0.0,
            power_watts: 0.0,
            inputs: vec![InputRequirement {
                resource_id: resource.to_string(),
                rate_kg_per_s: rate,
                upstream: None,
            }],
        });
    }

    // Use the first producer (TODO: let user choose)
    let (building, output_rate) = &producers[0];

    // Calculate how many buildings needed
    let num_buildings = rate / output_rate;
    let total_power = num_buildings * building.power_watts;

    // Get inputs for this building
    let inputs = db::get_building_inputs(conn, &building.id)?;

    // Recursively calculate upstream requirements
    let mut input_requirements = Vec::new();
    for input in inputs {
        let required_rate = input.rate_kg_per_s * num_buildings;

        // Try to find upstream producer
        let upstream = match calculate_chain_recursive(conn, &input.resource_id, required_rate, depth + 1) {
            Ok(node) => Some(Box::new(node)),
            Err(_) => None, // Raw resource or error
        };

        input_requirements.push(InputRequirement {
            resource_id: input.resource_id,
            rate_kg_per_s: required_rate,
            upstream,
        });
    }

    Ok(ProductionNode {
        building_id: building.id.clone(),
        building_name: building.name.clone(),
        count: num_buildings,
        power_watts: total_power,
        inputs: input_requirements,
    })
}

/// Calculate total power consumption for an entire production chain
pub fn total_power(node: &ProductionNode) -> f64 {
    let mut total = node.power_watts;
    for input in &node.inputs {
        if let Some(upstream) = &input.upstream {
            total += total_power(upstream);
        }
    }
    total
}

/// Format a production chain as a readable string
pub fn format_production_chain(node: &ProductionNode, indent: usize) -> String {
    let mut output = String::new();
    let prefix = "  ".repeat(indent);

    if node.building_id == "RAW_RESOURCE" {
        for input in &node.inputs {
            output.push_str(&format!(
                "{}→ {} @ {:.3} kg/s (raw input)\n",
                prefix, input.resource_id, input.rate_kg_per_s
            ));
        }
    } else {
        let power_str = if node.power_watts < 0.0 {
            format!("consumes {:.0}W", -node.power_watts)
        } else if node.power_watts > 0.0 {
            format!("generates {:.0}W", node.power_watts)
        } else {
            "no power".to_string()
        };

        output.push_str(&format!(
            "{}{:.2}x {} ({})\n",
            prefix, node.count, node.building_name, power_str
        ));

        for input in &node.inputs {
            output.push_str(&format!(
                "{}  needs {} @ {:.3} kg/s\n",
                prefix, input.resource_id, input.rate_kg_per_s
            ));
            if let Some(upstream) = &input.upstream {
                output.push_str(&format_production_chain(upstream, indent + 2));
            }
        }
    }

    output
}

/// Summary of a production chain calculation
#[derive(Debug)]
pub struct ChainSummary {
    pub target_resource: String,
    pub target_rate: f64,
    pub total_power_consumption: f64,
    pub total_power_generation: f64,
    pub net_power: f64,
    pub building_counts: Vec<(String, f64)>,
    pub raw_inputs: Vec<(String, f64)>,
}

/// Generate a summary of the production chain
pub fn summarize_chain(node: &ProductionNode, target_resource: &str, target_rate: f64) -> ChainSummary {
    let mut building_counts: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    let mut raw_inputs: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    let mut power_consumption = 0.0;
    let mut power_generation = 0.0;

    collect_summary(
        node,
        &mut building_counts,
        &mut raw_inputs,
        &mut power_consumption,
        &mut power_generation,
    );

    let mut building_list: Vec<_> = building_counts.into_iter().collect();
    building_list.sort_by(|a, b| a.0.cmp(&b.0));

    let mut raw_list: Vec<_> = raw_inputs.into_iter().collect();
    raw_list.sort_by(|a, b| a.0.cmp(&b.0));

    ChainSummary {
        target_resource: target_resource.to_string(),
        target_rate,
        total_power_consumption: power_consumption,
        total_power_generation: power_generation,
        net_power: power_generation - power_consumption,
        building_counts: building_list,
        raw_inputs: raw_list,
    }
}

fn collect_summary(
    node: &ProductionNode,
    buildings: &mut std::collections::HashMap<String, f64>,
    raw_inputs: &mut std::collections::HashMap<String, f64>,
    power_consumption: &mut f64,
    power_generation: &mut f64,
) {
    if node.building_id == "RAW_RESOURCE" {
        for input in &node.inputs {
            *raw_inputs.entry(input.resource_id.clone()).or_default() += input.rate_kg_per_s;
        }
    } else {
        *buildings.entry(node.building_name.clone()).or_default() += node.count;

        if node.power_watts < 0.0 {
            *power_consumption += -node.power_watts;
        } else {
            *power_generation += node.power_watts;
        }

        for input in &node.inputs {
            if let Some(upstream) = &input.upstream {
                collect_summary(upstream, buildings, raw_inputs, power_consumption, power_generation);
            } else {
                // No upstream producer - this is a raw input
                *raw_inputs.entry(input.resource_id.clone()).or_default() += input.rate_kg_per_s;
            }
        }
    }
}

impl std::fmt::Display for ChainSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "=== Production Summary ===")?;
        writeln!(f, "Target: {} @ {:.3} kg/s", self.target_resource, self.target_rate)?;
        writeln!(f)?;

        writeln!(f, "Buildings required:")?;
        for (name, count) in &self.building_counts {
            writeln!(f, "  {:.2}x {}", count, name)?;
        }
        writeln!(f)?;

        writeln!(f, "Raw inputs required:")?;
        for (name, rate) in &self.raw_inputs {
            writeln!(f, "  {} @ {:.3} kg/s", name, rate)?;
        }
        writeln!(f)?;

        writeln!(f, "Power:")?;
        writeln!(f, "  Consumption: {:.0}W", self.total_power_consumption)?;
        writeln!(f, "  Generation:  {:.0}W", self.total_power_generation)?;
        writeln!(f, "  Net:         {:.0}W", self.net_power)?;

        Ok(())
    }
}
