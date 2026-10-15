CREATE TABLE registry_games (
    game_id TEXT PRIMARY KEY NOT NULL,
    label TEXT NOT NULL
);
CREATE TABLE registry_editions (
    game_id TEXT NOT NULL REFERENCES registry_games(game_id) ON DELETE RESTRICT,
    edition_id TEXT NOT NULL,
    label TEXT NOT NULL,
    native INTEGER NOT NULL DEFAULT 0 CHECK (native IN (0,1)),
    PRIMARY KEY (game_id,edition_id),
    UNIQUE (game_id,label)
);
CREATE TABLE legacy_ruleset_aliases (
    system_id TEXT NOT NULL,
    edition TEXT NOT NULL,
    game_id TEXT NOT NULL,
    edition_id TEXT NOT NULL,
    PRIMARY KEY (system_id,edition),
    CHECK (system_id=game_id),
    FOREIGN KEY (game_id,edition_id) REFERENCES registry_editions(game_id,edition_id) ON DELETE RESTRICT
);
CREATE TABLE character_rulesets (
    character_id INTEGER PRIMARY KEY REFERENCES characters(id) ON DELETE CASCADE,
    game_id TEXT NOT NULL,
    edition_id TEXT NOT NULL,
    FOREIGN KEY (game_id,edition_id) REFERENCES registry_editions(game_id,edition_id) ON DELETE RESTRICT
);
CREATE TABLE source_rulesets (
    source_id INTEGER PRIMARY KEY REFERENCES sources(id) ON DELETE CASCADE,
    game_id TEXT NOT NULL,
    edition_id TEXT NOT NULL,
    FOREIGN KEY (game_id,edition_id) REFERENCES registry_editions(game_id,edition_id) ON DELETE RESTRICT
);
INSERT INTO registry_games VALUES ('mork-borg','MÖRK BORG');
INSERT INTO registry_editions VALUES ('mork-borg','it-2025','MÖRK BORG — IT 2025',1);
INSERT INTO legacy_ruleset_aliases VALUES ('mork-borg','MÖRK BORG — IT 2025','mork-borg','it-2025');
INSERT INTO character_rulesets SELECT c.id,a.game_id,a.edition_id FROM characters c JOIN legacy_ruleset_aliases a ON a.system_id=c.system_id AND a.edition=c.edition;
INSERT INTO source_rulesets SELECT s.id,a.game_id,a.edition_id FROM sources s JOIN legacy_ruleset_aliases a ON a.system_id=s.system_id AND a.edition=s.edition;
CREATE TRIGGER registry_games_immutable_update BEFORE UPDATE ON registry_games BEGIN SELECT RAISE(ABORT,'Registry identities and labels are immutable'); END;
CREATE TRIGGER registry_games_immutable_delete BEFORE DELETE ON registry_games BEGIN SELECT RAISE(ABORT,'Registry identities are retained'); END;
CREATE TRIGGER registry_editions_immutable_update BEFORE UPDATE ON registry_editions BEGIN SELECT RAISE(ABORT,'Registry editions are immutable'); END;
CREATE TRIGGER registry_editions_immutable_delete BEFORE DELETE ON registry_editions BEGIN SELECT RAISE(ABORT,'Registry editions are retained'); END;
CREATE TRIGGER ruleset_aliases_immutable_update BEFORE UPDATE ON legacy_ruleset_aliases BEGIN SELECT RAISE(ABORT,'Ruleset aliases are immutable'); END;
CREATE TRIGGER ruleset_aliases_immutable_delete BEFORE DELETE ON legacy_ruleset_aliases BEGIN SELECT RAISE(ABORT,'Ruleset aliases are retained'); END;
PRAGMA user_version = 5;
