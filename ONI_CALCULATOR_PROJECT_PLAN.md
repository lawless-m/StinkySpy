# Oxygen Not Included Production Calculator

## Project Overview

Build a production chain calculator for Oxygen Not Included that helps players determine:
- How many buildings are needed to achieve a target production rate
- Total power consumption for a production setup
- Resource flow requirements (inputs/outputs in kg/s)
- Upstream/downstream dependencies

## Architecture

### Database: SQLite

Single bundled database file containing all game data. Schema:

```sql
-- Core element/resource data
CREATE TABLE resources (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    state TEXT,  -- Solid, Liquid, Gas
    specific_heat_capacity REAL,
    thermal_conductivity REAL,
    melt_point_c REAL,
    boil_point_c REAL
);

-- Building definitions
CREATE TABLE buildings (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    category TEXT,
    power_watts INTEGER,  -- Negative = consumes, Positive = generates
    heat_output_dtu REAL,
    construction_time_s REAL
);

-- Building material requirements
CREATE TABLE building_materials (
    building_id TEXT REFERENCES buildings(id),
    resource_id TEXT REFERENCES resources(id),
    mass_kg REAL,
    PRIMARY KEY (building_id, resource_id)
);

-- Production inputs (what a building consumes)
CREATE TABLE building_inputs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    building_id TEXT REFERENCES buildings(id),
    resource_id TEXT REFERENCES resources(id),
    rate_kg_per_s REAL NOT NULL
);

-- Production outputs (what a building produces)
CREATE TABLE building_outputs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    building_id TEXT REFERENCES buildings(id),
    resource_id TEXT REFERENCES resources(id),
    rate_kg_per_s REAL NOT NULL
);

-- Some buildings have multiple operational modes (e.g., Metal Refinery recipes)
CREATE TABLE recipes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    building_id TEXT REFERENCES buildings(id),
    name TEXT NOT NULL
);

CREATE TABLE recipe_inputs (
    recipe_id INTEGER REFERENCES recipes(id),
    resource_id TEXT REFERENCES resources(id),
    rate_kg_per_s REAL NOT NULL,
    PRIMARY KEY (recipe_id, resource_id)
);

CREATE TABLE recipe_outputs (
    recipe_id INTEGER REFERENCES recipes(id),
    resource_id TEXT REFERENCES resources(id),
    rate_kg_per_s REAL NOT NULL,
    PRIMARY KEY (recipe_id, resource_id)
);
```

---

## Phase 1: Data Extraction from Game Files

### Step 1.1: Obtain dnSpyEx

Download the actively maintained fork from: https://github.com/dnSpyEx/dnSpy/releases

Install/extract to a working directory.

### Step 1.2: Locate the Game DLL

Find the main game code at:
```
<Steam Directory>/steamapps/common/OxygenNotIncluded/OxygenNotIncluded_Data/Managed/Assembly-CSharp.dll
```

### Step 1.3: Export Decompiled Source

1. Open dnSpy
2. File → Open → Select `Assembly-CSharp.dll`
3. Right-click the `Assembly-CSharp` node in the tree
4. Select "Export to Project..."
5. Choose C# project format
6. Export to a directory (e.g., `oni-decompiled/`)

This will produce a folder full of `.cs` files containing all game logic.

### Step 1.4: Identify Key Code Patterns

Building definitions follow a consistent pattern. Look for:

**Building Configs** — Classes ending in `Config` that implement `IBuildingConfig`:
- `ElectrolyzerConfig.cs`
- `MetalRefineryConfig.cs`
- `GasGeneratorConfig.cs`
- etc.

**Key Methods to Parse**:

```csharp
// In CreateBuildingDef():
BuildingDef buildingDef = BuildingTemplates.CreateBuildingDef(
    "Electrolyzer",      // ID
    1, 2,                // Width, Height
    "electrolyzer_kanim",
    30,                  // HP
    30f,                 // Construction time
    BUILDINGS.CONSTRUCTION_MASS_KG.TIER3,  // Material mass
    MATERIALS.ALL_METALS,                   // Material type
    800f,                // Melting point
    ...
);
buildingDef.RequiresPowerInput = true;
buildingDef.EnergyConsumptionWhenActive = 120f;  // <-- POWER

// In ConfigureBuildingTemplate():
ElementConverter converter = go.AddOrGet<ElementConverter>();
converter.consumedElements = new ElementConverter.ConsumedElement[]
{
    new ElementConverter.ConsumedElement(SimHashes.Water, 1f)  // <-- INPUT: 1 kg/s water
};
converter.outputElements = new ElementConverter.OutputElement[]
{
    new ElementConverter.OutputElement(0.888f, SimHashes.Oxygen, ...),   // <-- OUTPUT
    new ElementConverter.OutputElement(0.112f, SimHashes.Hydrogen, ...)  // <-- OUTPUT
};
```

**Element Definitions** — Look for `ElementLoader` or files defining `SimHashes`:
- Element properties (specific heat, state, transitions)
- These map the `SimHashes` enum values to actual element data

### Step 1.5: Write Extraction Script

Create a Python script to parse the decompiled `.cs` files:

```python
#!/usr/bin/env python3
"""
ONI Building Data Extractor

Parses decompiled C# source from Assembly-CSharp.dll to extract
building definitions, inputs, outputs, and power requirements.
"""

import os
import re
import sqlite3
import json
from pathlib import Path
from dataclasses import dataclass, field
from typing import Optional

@dataclass
class ConsumedElement:
    element: str
    rate_kg_s: float

@dataclass
class OutputElement:
    element: str
    rate_kg_s: float

@dataclass 
class Building:
    id: str
    name: str = ""
    power_watts: float = 0
    heat_dtu: float = 0
    inputs: list[ConsumedElement] = field(default_factory=list)
    outputs: list[OutputElement] = field(default_factory=list)

def find_config_files(decompiled_dir: Path) -> list[Path]:
    """Find all *Config.cs files that likely define buildings."""
    configs = []
    for cs_file in decompiled_dir.rglob("*Config.cs"):
        content = cs_file.read_text(errors='ignore')
        if "IBuildingConfig" in content or "CreateBuildingDef" in content:
            configs.append(cs_file)
    return configs

def parse_building_config(filepath: Path) -> Optional[Building]:
    """
    Parse a single building config file.
    Extract: ID, power consumption, inputs, outputs.
    """
    content = filepath.read_text(errors='ignore')
    
    building = None
    
    # Extract building ID from CreateBuildingDef call
    # Pattern: CreateBuildingDef("BuildingID", ...)
    id_match = re.search(r'CreateBuildingDef\s*\(\s*"(\w+)"', content)
    if id_match:
        building = Building(id=id_match.group(1))
    else:
        return None
    
    # Extract power consumption
    # Pattern: EnergyConsumptionWhenActive = 120f
    power_match = re.search(r'EnergyConsumptionWhenActive\s*=\s*([\d.]+)f?', content)
    if power_match:
        building.power_watts = -float(power_match.group(1))  # Negative = consumption
    
    # Check for power generation
    # Pattern: GeneratorWattageRating = 800f
    gen_match = re.search(r'GeneratorWattageRating\s*=\s*([\d.]+)f?', content)
    if gen_match:
        building.power_watts = float(gen_match.group(1))  # Positive = generation
    
    # Extract consumed elements
    # Pattern: new ElementConverter.ConsumedElement(SimHashes.Water, 1f)
    consumed_pattern = r'ConsumedElement\s*\(\s*(?:SimHashes\.)?(\w+)\s*,\s*([\d.]+)f?\s*\)'
    for match in re.finditer(consumed_pattern, content):
        element = match.group(1)
        rate = float(match.group(2))
        building.inputs.append(ConsumedElement(element, rate))
    
    # Extract output elements  
    # Pattern: new ElementConverter.OutputElement(0.888f, SimHashes.Oxygen, ...)
    output_pattern = r'OutputElement\s*\(\s*([\d.]+)f?\s*,\s*(?:SimHashes\.)?(\w+)'
    for match in re.finditer(output_pattern, content):
        rate = float(match.group(1))
        element = match.group(2)
        building.outputs.append(OutputElement(element, rate))
    
    return building

def create_database(buildings: list[Building], db_path: Path):
    """Create SQLite database with extracted building data."""
    conn = sqlite3.connect(db_path)
    cur = conn.cursor()
    
    # Create tables
    cur.executescript("""
        CREATE TABLE IF NOT EXISTS buildings (
            id TEXT PRIMARY KEY,
            name TEXT,
            power_watts REAL,
            heat_output_dtu REAL
        );
        
        CREATE TABLE IF NOT EXISTS building_inputs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            building_id TEXT REFERENCES buildings(id),
            resource_id TEXT,
            rate_kg_per_s REAL
        );
        
        CREATE TABLE IF NOT EXISTS building_outputs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            building_id TEXT REFERENCES buildings(id),
            resource_id TEXT,
            rate_kg_per_s REAL
        );
    """)
    
    for b in buildings:
        cur.execute(
            "INSERT OR REPLACE INTO buildings (id, name, power_watts) VALUES (?, ?, ?)",
            (b.id, b.name or b.id, b.power_watts)
        )
        for inp in b.inputs:
            cur.execute(
                "INSERT INTO building_inputs (building_id, resource_id, rate_kg_per_s) VALUES (?, ?, ?)",
                (b.id, inp.element, inp.rate_kg_s)
            )
        for out in b.outputs:
            cur.execute(
                "INSERT INTO building_outputs (building_id, resource_id, rate_kg_per_s) VALUES (?, ?, ?)",
                (b.id, out.element, out.rate_kg_s)
            )
    
    conn.commit()
    conn.close()

def main():
    import argparse
    parser = argparse.ArgumentParser(description="Extract ONI building data")
    parser.add_argument("decompiled_dir", type=Path, help="Path to decompiled source")
    parser.add_argument("-o", "--output", type=Path, default=Path("oni_data.db"))
    args = parser.parse_args()
    
    print(f"Scanning {args.decompiled_dir} for building configs...")
    config_files = find_config_files(args.decompiled_dir)
    print(f"Found {len(config_files)} potential building config files")
    
    buildings = []
    for cf in config_files:
        building = parse_building_config(cf)
        if building:
            buildings.append(building)
            print(f"  Parsed: {building.id} (power: {building.power_watts}W, "
                  f"inputs: {len(building.inputs)}, outputs: {len(building.outputs)})")
    
    print(f"\nExtracted {len(buildings)} buildings")
    print(f"Writing to {args.output}...")
    create_database(buildings, args.output)
    print("Done!")

if __name__ == "__main__":
    main()
```

**Usage:**
```bash
python extract_oni_data.py ./oni-decompiled/ -o oni_data.db
```

### Step 1.6: Manual Verification & Cleanup

The regex-based extraction won't catch everything perfectly. After initial extraction:

1. Query the database to find buildings with missing data
2. Manually check those `.cs` files for edge cases
3. Some buildings use different patterns (e.g., `EnergyGenerator`, `ElementConsumer` instead of `ElementConverter`)
4. Add handling for recipes (Metal Refinery, Rock Crusher, etc. have multiple modes)

---

## Phase 2: Calculator Application

### Technology Choice

Recommend one of:
- **Python + CLI**: Simple, SQLite works natively, easy to iterate
- **Python + Textual**: Nice TUI if you want something fancier
- **Rust + SQLite**: If bundling as a single binary matters
- **Web (HTML/JS + sql.js)**: If you want a browser-based tool

### Core Calculator Logic

```python
def calculate_production_chain(
    db: sqlite3.Connection,
    target_resource: str,
    target_rate_kg_s: float
) -> dict:
    """
    Given a target output, calculate all required buildings and inputs.
    Returns a tree of requirements.
    """
    
    # Find buildings that produce this resource
    cur = db.execute("""
        SELECT b.id, b.power_watts, bo.rate_kg_per_s
        FROM buildings b
        JOIN building_outputs bo ON b.id = bo.building_id
        WHERE bo.resource_id = ?
    """, (target_resource,))
    
    producers = cur.fetchall()
    if not producers:
        return {"error": f"No building produces {target_resource}"}
    
    # Pick the first producer (could let user choose)
    building_id, power_watts, output_rate = producers[0]
    
    # Calculate how many buildings needed
    num_buildings = target_rate_kg_s / output_rate
    total_power = num_buildings * power_watts
    
    # Get inputs for this building
    inputs = db.execute("""
        SELECT resource_id, rate_kg_per_s
        FROM building_inputs
        WHERE building_id = ?
    """, (building_id,)).fetchall()
    
    # Recursively calculate upstream requirements
    upstream = {}
    for resource_id, input_rate in inputs:
        required_rate = input_rate * num_buildings
        upstream[resource_id] = calculate_production_chain(
            db, resource_id, required_rate
        )
    
    return {
        "building": building_id,
        "count": num_buildings,
        "power_watts": total_power,
        "inputs": upstream
    }
```

### Example Queries the Calculator Should Support

1. **"I want 1 kg/s of oxygen"**
   → 1.126 Electrolyzers, consuming 1.126 kg/s water, using 135W

2. **"I want to run 8 dupes on oxygen"**
   → Convert dupe O2 consumption (100g/s each) → 0.8 kg/s → calculate from there

3. **"What does a full petroleum boiler need?"**
   → Show entire production chain with all intermediates

4. **"Power budget for steel production at X kg/cycle"**
   → Sum power across entire chain

---

## Phase 3: Enhancements (Future)

- **Dupe labour**: Factor in operator time for buildings that need it
- **Pipe throughput**: Flag when production exceeds 10 kg/s per pipe
- **Heat management**: Calculate DTU output for thermal planning
- **Recipes**: Support buildings with multiple modes (refinery, kiln, etc.)
- **Import/export**: Save/load production setups
- **Visualisation**: Render the production graph

---

## File Structure

```
oni-calculator/
├── data/
│   ├── extract_oni_data.py    # Extraction script
│   └── oni_data.db            # Generated database
├── src/
│   ├── calculator.py          # Core calculation logic
│   ├── db.py                   # Database helpers
│   └── cli.py                  # Command-line interface
├── oni-decompiled/             # Decompiled game source (gitignore this)
└── README.md
```

---

## Quick Start for Claude Code

1. **Get the decompiled source**: User will provide or you help them export from dnSpy
2. **Run extraction**: `python data/extract_oni_data.py ./oni-decompiled/ -o data/oni_data.db`
3. **Verify data**: Query the DB to check extraction worked
4. **Build calculator**: Implement the calculation logic and CLI
5. **Iterate**: Fix extraction regex for edge cases as discovered

---

## Known Challenges

- **Regex limitations**: The C# patterns vary; some buildings use different component types
- **Recipes**: Buildings like Metal Refinery have multiple recipes defined elsewhere
- **Element names**: Need to map `SimHashes` enum to human-readable names
- **DLC content**: Spaced Out! adds more buildings; check which DLL to decompile
- **Tuning constants**: Some values are in `TUNING` classes, not the config files

---

## Resources

- ONI Wiki: https://oxygennotincluded.wiki.gg/
- ONI Database (community): https://oni-db.com/
- dnSpyEx: https://github.com/dnSpyEx/dnSpy
- Game files guide: https://oxygennotincluded.wiki.gg/wiki/Guide/Working_with_the_Game_Files
