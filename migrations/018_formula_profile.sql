-- Formula profile 1 (CEL "cel-mimik-1"): definitions may now carry derived-field formulas,
-- list fields and advisory constraints as additive members of contract 1. No table changes:
-- the version bump makes older binaries refuse a database that may hold such definitions.
PRAGMA user_version=18;
