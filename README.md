# StinkySpy

Oxygen Not Included tools and Claude Code skills.

## ONI Calculator

Production chain calculator for Oxygen Not Included, built in Rust with SQLite.

Extracts building data directly from decompiled game source - actual game values, not wiki approximations.

### Features

- Building power consumption/generation
- Input/output rates (kg/s)
- Heat output (DTU/s)
- Production chain calculations

### Usage

```bash
cd oni-calculator

# Extract from decompiled source
cargo run -- extract ../oni-decompiled/Assembly-CSharp --clear

# Query buildings
cargo run -- building Electrolyzer
cargo run -- list-buildings
cargo run -- list-resources

# Calculate production chains
cargo run -- calc Oxygen --rate 1.0
```

### Best Interface

Ask Claude. The database is the backend, Claude is the query interface:

> "How many electrolyzers for a hydrogen generator?"
> "What produces CO2?"
> "Which generator runs coolest per watt?"

## Skills

Claude Code skill documents in `.claude/skills/`:

- **ONI Calculator** - How to query the database and fix extraction patterns
- **Skill Building** - How to create new skill documents
- **Databases** - RDBMS access patterns
- **Logging** - UTF-8 file logging for services
