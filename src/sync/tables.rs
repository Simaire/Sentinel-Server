//! Tables partagées entre les applis Sentinel : la même définition que dans l'appli
//! (`src-tauri/src/db/tables.rs`), plus les règles de propriété propres au serveur.
//!
//! Chaque ligne porte `seq`, un numéro donné par le serveur à chaque écriture (séquence
//! unique et croissante) : un client ne relit que les lignes dont `seq` dépasse son
//! curseur, sans dépendre des horloges des PC.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    Int,
}

/// Fusion d'une colonne quand deux versions de la ligne se rencontrent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Merge {
    Key,
    /// Version la plus récente, sans effacer une valeur connue par une valeur vide.
    Recent,
    /// Version la plus récente, même vide.
    Latest,
    Max,
    Min,
}

#[derive(Debug)]
pub struct Column {
    pub name: &'static str,
    pub kind: Kind,
    pub merge: Merge,
    /// Longueur maximale d'un texte (octets).
    pub max_len: usize,
}

/// Colonne qui désigne l'auteur de la ligne : elle doit être le compte connecté.
#[derive(Debug)]
pub struct Owner {
    pub id_column: &'static str,
    /// Colonne du pseudo de l'auteur, réécrite avec le pseudo vérifié du compte.
    pub name_column: Option<&'static str>,
    /// Une ligne existante d'un autre auteur ne peut pas être modifiée (sauf admin).
    pub protect_existing: bool,
    /// La colonne peut rester vide (observation anonyme).
    pub optional: bool,
}

#[derive(Debug)]
pub struct Table {
    pub name: &'static str,
    pub columns: &'static [Column],
    pub owner: Option<Owner>,
}

const SHORT: usize = 512;
const TEXT: usize = 4_096;
const LONG: usize = 262_144;

const fn text(name: &'static str, merge: Merge) -> Column {
    Column { name, kind: Kind::Text, merge, max_len: TEXT }
}
const fn key(name: &'static str) -> Column {
    Column { name, kind: Kind::Text, merge: Merge::Key, max_len: SHORT }
}
const fn long(name: &'static str, merge: Merge) -> Column {
    Column { name, kind: Kind::Text, merge, max_len: LONG }
}
const fn int(name: &'static str, merge: Merge) -> Column {
    Column { name, kind: Kind::Int, merge, max_len: 0 }
}

use Merge::{Latest, Max, Min, Recent};

pub static TABLES: &[Table] = &[
    Table {
        name: "avatars",
        columns: &[
            key("name"),
            text("avatar_id", Recent),
            text("id_source", Recent),
            text("first_seen", Min),
            text("last_seen", Max),
            int("wear_count", Max),
            long("api_json", Recent),
            long("api_raw", Recent),
            int("api_checked_at", Max),
            text("api_unavailable", Latest),
            long("profile_tags", Recent),
            int("external_checked_at", Max),
            int("external_known", Recent),
            int("contributed_at", Max),
            text("external_thumbnail", Recent),
            long("analysis_json", Recent),
            text("analysis_key", Recent),
            int("analysis_checked_at", Max),
            text("analysis_unavailable", Latest),
            text("log_author", Recent),
            int("updated_at", Max),
        ],
        owner: None,
    },
    Table {
        name: "avatar_wearers",
        columns: &[
            key("avatar_name"),
            key("wearer_key"),
            text("user_id", Recent),
            text("display_name", Recent),
            text("last_seen", Max),
            int("wear_count", Max),
            int("updated_at", Max),
        ],
        owner: None,
    },
    Table {
        name: "avatar_crashes",
        columns: &[key("avatar_name"), key("log_file"), text("crashed_at", Min), int("updated_at", Max)],
        owner: None,
    },
    Table {
        name: "players",
        columns: &[
            key("id"),
            text("display_name", Recent),
            text("trust_rank", Recent),
            long("tags", Recent),
            text("age_verification", Recent),
            long("profile_json", Recent),
            int("profile_fetched_at", Max),
            int("first_seen_at", Min),
            int("last_seen_at", Max),
            int("updated_at", Max),
        ],
        owner: None,
    },
    Table {
        name: "player_sightings",
        columns: &[
            key("user_id"),
            key("location"),
            key("joined_at"),
            text("display_name", Recent),
            text("left_at", Max),
            int("updated_at", Max),
        ],
        owner: None,
    },
    Table {
        name: "reports",
        columns: &[
            key("id"),
            text("user_id", Recent),
            text("display_name", Recent),
            text("category", Recent),
            text("note", Recent),
            text("avatar_name", Recent),
            text("location", Recent),
            text("reporter_id", Recent),
            text("reporter_name", Recent),
            int("created_at", Min),
            int("deleted", Max),
            int("updated_at", Max),
            // Suivi par l'équipe jusqu'à cette date (Unix) ; 0 : pas de suivi.
            int("watch_until", Recent),
        ],
        owner: Some(Owner {
            id_column: "reporter_id",
            name_column: Some("reporter_name"),
            protect_existing: true,
            optional: false,
        }),
    },
    Table {
        name: "votes",
        columns: &[
            key("target_type"),
            key("target_id"),
            key("voter_id"),
            text("voter_name", Recent),
            int("value", Latest),
            int("updated_at", Max),
        ],
        owner: Some(Owner {
            id_column: "voter_id",
            name_column: Some("voter_name"),
            protect_existing: false,
            optional: false,
        }),
    },
    Table {
        name: "vote_kicks",
        columns: &[
            key("target_key"),
            key("location"),
            key("initiator_key"),
            text("target_id", Recent),
            text("target_name", Recent),
            text("initiator_id", Recent),
            text("initiator_name", Recent),
            text("kicked_at", Min),
            int("left_soon", Max),
            text("observer_id", Min),
            int("updated_at", Max),
        ],
        owner: Some(Owner {
            id_column: "observer_id",
            name_column: None,
            protect_existing: false,
            optional: true,
        }),
    },
    // Spam visuel (portails, stickers, émojis, prints), depuis l'appli 0.1.3.
    Table {
        name: "visual_spam",
        columns: &[
            key("user_key"),
            key("location"),
            key("kind"),
            text("user_id", Recent),
            text("display_name", Recent),
            int("spawns", Max),
            int("burst", Max),
            text("first_at", Min),
            text("last_at", Max),
            text("observer_id", Min),
            int("updated_at", Max),
        ],
        owner: Some(Owner {
            id_column: "observer_id",
            name_column: None,
            protect_existing: false,
            optional: true,
        }),
    },
    Table {
        name: "alerts",
        columns: &[
            key("id"),
            text("observer_id", Recent),
            text("observer_name", Recent),
            text("level", Recent),
            text("title", Recent),
            text("body", Recent),
            text("user_id", Recent),
            text("location", Recent),
            int("created_at", Min),
            int("updated_at", Max),
        ],
        owner: Some(Owner {
            id_column: "observer_id",
            name_column: Some("observer_name"),
            protect_existing: true,
            optional: false,
        }),
    },
    Table {
        name: "moderation_actions",
        columns: &[
            key("id"),
            text("actor_id", Recent),
            text("actor_name", Recent),
            text("target_id", Recent),
            text("action", Recent),
            text("group_id", Recent),
            int("ok", Max),
            text("detail", Recent),
            int("created_at", Min),
            int("updated_at", Max),
            // Raison donnée par le modo (obligatoire pour une sanction, depuis l'appli 0.1.3).
            text("reason", Recent),
        ],
        owner: Some(Owner {
            id_column: "actor_id",
            name_column: Some("actor_name"),
            protect_existing: true,
            optional: false,
        }),
    },
    Table {
        name: "vrc_groups",
        columns: &[
            key("group_id"),
            text("name", Recent),
            text("short_code", Recent),
            int("member_count", Recent),
            int("updated_at", Max),
        ],
        owner: None,
    },
    Table {
        name: "player_groups",
        columns: &[
            key("user_id"),
            key("group_id"),
            int("member", Recent),
            int("banned", Recent),
            int("is_moderator", Recent),
            int("warns", Max),
            int("kicks", Max),
            int("bans", Max),
            int("updated_at", Max),
        ],
        owner: None,
    },
    Table {
        name: "moderator_groups",
        columns: &[
            key("moderator_id"),
            key("group_id"),
            text("group_name", Recent),
            int("active", Latest),
            int("updated_at", Max),
        ],
        owner: Some(Owner {
            id_column: "moderator_id",
            name_column: None,
            protect_existing: false,
            optional: false,
        }),
    },
];

pub fn table(name: &str) -> Option<&'static Table> {
    TABLES.iter().find(|t| t.name == name)
}

impl Table {
    pub fn column_names(&self) -> Vec<&'static str> {
        self.columns.iter().map(|c| c.name).collect()
    }

    pub fn index_of(&self, column: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == column)
    }

    pub fn key_indexes(&self) -> Vec<usize> {
        self.columns
            .iter()
            .enumerate()
            .filter(|(_, c)| c.merge == Merge::Key)
            .map(|(i, _)| i)
            .collect()
    }

    pub fn keys(&self) -> Vec<&'static str> {
        self.columns.iter().filter(|c| c.merge == Merge::Key).map(|c| c.name).collect()
    }
}

fn sql_type(kind: Kind) -> &'static str {
    match kind {
        Kind::Text => "TEXT",
        Kind::Int => "BIGINT",
    }
}

/// Création et mise à niveau de la table : colonnes ajoutées sans perte.
pub fn schema_sql(table: &Table) -> Vec<String> {
    let t = table.name;
    let columns: Vec<String> = table
        .columns
        .iter()
        .map(|c| {
            let not_null = if c.merge == Merge::Key { " NOT NULL" } else { "" };
            format!("{} {}{not_null}", c.name, sql_type(c.kind))
        })
        .collect();
    let mut statements = vec![format!(
        "CREATE TABLE IF NOT EXISTS {t} ({}, seq BIGINT NOT NULL, updated_by TEXT, PRIMARY KEY ({}))",
        columns.join(", "),
        table.keys().join(", ")
    )];
    for c in table.columns {
        statements.push(format!(
            "ALTER TABLE {t} ADD COLUMN IF NOT EXISTS {} {}",
            c.name,
            sql_type(c.kind)
        ));
    }
    statements.push(format!("CREATE INDEX IF NOT EXISTS {t}_seq ON {t} (seq)"));
    statements
}

fn merge_expr(table: &str, column: &Column) -> Option<String> {
    let (t, c) = (table, column.name);
    Some(match column.merge {
        Merge::Key => return None,
        Merge::Recent => format!(
            "{c} = CASE WHEN excluded.updated_at >= {t}.updated_at OR {t}.updated_at IS NULL \
             THEN COALESCE(excluded.{c}, {t}.{c}) ELSE COALESCE({t}.{c}, excluded.{c}) END"
        ),
        Merge::Latest => format!(
            "{c} = CASE WHEN excluded.updated_at >= {t}.updated_at OR {t}.updated_at IS NULL \
             THEN excluded.{c} ELSE {t}.{c} END"
        ),
        Merge::Max => format!("{c} = CASE WHEN {t}.{c} IS NULL OR excluded.{c} > {t}.{c} THEN excluded.{c} ELSE {t}.{c} END"),
        Merge::Min => format!("{c} = CASE WHEN {t}.{c} IS NULL OR excluded.{c} < {t}.{c} THEN excluded.{c} ELSE {t}.{c} END"),
    })
}

/// Écriture groupée : une seule requête pour toutes les lignes d'une table (`UNNEST` de
/// tableaux typés), fusionnées avec l'existant. Paramètres : une colonne par tableau,
/// puis l'auteur (`$n+1`). Chaque ligne reçoit un nouveau `seq`.
pub fn bulk_upsert_sql(table: &Table) -> String {
    let n = table.columns.len();
    let names = table.column_names().join(", ");
    let arrays: Vec<String> = table
        .columns
        .iter()
        .enumerate()
        .map(|(i, c)| format!("${}::{}[]", i + 1, sql_type(c.kind).to_lowercase()))
        .collect();
    let mut sets: Vec<String> = table.columns.iter().filter_map(|c| merge_expr(table.name, c)).collect();
    sets.push("seq = excluded.seq".into());
    sets.push("updated_by = excluded.updated_by".into());
    format!(
        "INSERT INTO {t} ({names}, seq, updated_by)
         SELECT u.*, nextval('sync_seq'), ${author} FROM UNNEST({arrays}) AS u({names})
         ON CONFLICT ({keys}) DO UPDATE SET {sets}",
        t = table.name,
        author = n + 1,
        arrays = arrays.join(", "),
        keys = table.keys().join(", "),
        sets = sets.join(", ")
    )
}

/// Lignes modifiées après le curseur, dans l'ordre des écritures.
pub fn pull_sql(table: &Table) -> String {
    format!(
        "SELECT {}, seq FROM {} WHERE seq > $1 ORDER BY seq LIMIT $2",
        table.column_names().join(", "),
        table.name
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_table_has_keys_and_updated_at() {
        for t in TABLES {
            assert!(!t.keys().is_empty(), "{} sans clé", t.name);
            assert!(t.index_of("updated_at").is_some(), "{} sans updated_at", t.name);
            if let Some(owner) = &t.owner {
                assert!(t.index_of(owner.id_column).is_some());
                if let Some(name) = owner.name_column {
                    assert!(t.index_of(name).is_some());
                }
            }
        }
    }

    #[test]
    fn bulk_sql_uses_typed_arrays_and_merges() {
        let votes = table("votes").unwrap();
        let sql = bulk_upsert_sql(votes);
        assert!(sql.contains("UNNEST($1::text[], $2::text[], $3::text[], $4::text[], $5::bigint[], $6::bigint[])"));
        assert!(sql.contains("nextval('sync_seq'), $7"));
        assert!(sql.contains("ON CONFLICT (target_type, target_id, voter_id)"));
        assert!(sql.contains("value = CASE WHEN excluded.updated_at >= votes.updated_at"));
        assert!(schema_sql(votes)[0].contains("PRIMARY KEY (target_type, target_id, voter_id)"));
    }
}
