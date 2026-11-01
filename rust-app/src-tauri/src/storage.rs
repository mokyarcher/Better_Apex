use anyhow::{ensure, Context, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};

const NEW_SCHEMA_VERSION: i64 = 1;
const NEW_APPLICATION_ID: i64 = 0x4241_5058; // "BAPX"

#[derive(Clone)]
pub struct LocalStore {
    data_dir: PathBuf,
    database_path: PathBuf,
}

#[derive(Serialize)]
pub struct LocalStoreStatus {
    pub data_dir: String,
    pub database_path: String,
    pub record_count: i64,
    pub schema_version: i64,
}

#[derive(Clone, Serialize)]
pub struct SavedPlayer {
    pub slot: i64,
    pub nickname: Option<String>,
    pub name_confirmed: bool,
    pub is_local: Option<bool>,
    pub kills: Option<i64>,
    pub assists: Option<i64>,
    pub knocks: Option<i64>,
    pub damage: Option<i64>,
}

#[derive(Clone, Serialize)]
pub struct SavedMatch {
    pub id: String,
    pub saved_at: String,
    pub rank: Option<i64>,
    pub squad_kills: Option<i64>,
    pub players: Vec<SavedPlayer>,
    pub has_settlement_image: bool,
    pub has_selection_image: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NicknameSource {
    /// Automatic 02 HUD roster. The serialized name is retained for existing data;
    /// current recording uses one read, without voting.
    VotedRoster,
    Manual,
    #[default]
    Unresolved,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct PlayerAnnotation {
    pub nickname: Option<String>,
    pub name_confirmed: bool,
    pub is_local: Option<bool>,
    #[serde(default)]
    pub nickname_source: NicknameSource,
}

#[derive(Serialize)]
pub struct SaveMatchResult {
    pub id: String,
    pub saved_at: String,
    pub deduplicated: bool,
}

impl LocalStore {
    pub fn open(data_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&data_dir)?;
        let database_path = data_dir.join("matches.db");
        let connection = Connection::open(&database_path)?;
        connection.pragma_update(None, "foreign_keys", true)?;

        let schema_version: i64 =
            connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let application_id: i64 =
            connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
        ensure!(
            schema_version <= NEW_SCHEMA_VERSION,
            "新版战绩数据库版本较新，请使用更新版本的 Better Apex"
        );

        if schema_version == 0 {
            ensure!(
                application_id == 0 || application_id == NEW_APPLICATION_ID,
                "数据目录中的数据库属于其他应用，拒绝修改"
            );
            let table_count: i64 = connection.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get(0),
            )?;
            ensure!(
                table_count == 0,
                "数据目录中的数据库已有未知表结构，拒绝修改"
            );
            connection.execute_batch(&format!(
                "BEGIN IMMEDIATE;
                 CREATE TABLE matches (
                    id TEXT PRIMARY KEY NOT NULL,
                    saved_at_utc TEXT NOT NULL,
                    rank INTEGER CHECK (rank IS NULL OR rank >= 1),
                    squad_kills INTEGER CHECK (squad_kills IS NULL OR squad_kills >= 0)
                 );
                 CREATE TABLE players (
                    match_id TEXT NOT NULL REFERENCES matches(id) ON DELETE CASCADE,
                    slot INTEGER NOT NULL CHECK (slot BETWEEN 0 AND 2),
                    nickname TEXT,
                    name_confirmed INTEGER NOT NULL DEFAULT 0 CHECK (name_confirmed IN (0, 1)),
                    is_local INTEGER CHECK (is_local IS NULL OR is_local IN (0, 1)),
                    kills INTEGER CHECK (kills IS NULL OR kills >= 0),
                    assists INTEGER CHECK (assists IS NULL OR assists >= 0),
                    knocks INTEGER CHECK (knocks IS NULL OR knocks >= 0),
                    damage INTEGER CHECK (damage IS NULL OR damage >= 0),
                    PRIMARY KEY (match_id, slot)
                 );
                 CREATE TABLE match_images (
                    match_id TEXT NOT NULL REFERENCES matches(id) ON DELETE CASCADE,
                    kind TEXT NOT NULL CHECK (kind IN ('selection', 'settlement')),
                    relative_path TEXT NOT NULL,
                    sha256 TEXT,
                    PRIMARY KEY (match_id, kind)
                 );
                 CREATE INDEX matches_saved_at ON matches(saved_at_utc DESC, id DESC);
                 PRAGMA application_id={NEW_APPLICATION_ID};
                 PRAGMA user_version={NEW_SCHEMA_VERSION};
                 COMMIT;"
            ))?;
        } else {
            ensure!(
                application_id == NEW_APPLICATION_ID,
                "新版数据库标识不匹配，拒绝访问"
            );
        }

        Ok(Self {
            data_dir,
            database_path,
        })
    }

    pub fn status(&self) -> Result<LocalStoreStatus> {
        let connection = Connection::open(&self.database_path)?;
        connection.pragma_update(None, "foreign_keys", true)?;
        let record_count =
            connection.query_row("SELECT count(*) FROM matches", [], |row| row.get(0))?;
        let schema_version: i64 =
            connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        Ok(LocalStoreStatus {
            data_dir: self.data_dir.display().to_string(),
            database_path: self.database_path.display().to_string(),
            record_count,
            schema_version,
        })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn save_settlement(
        &self,
        settlement: &crate::settlement::SettlementResult,
        source_image: &Path,
        annotations: &[PlayerAnnotation],
        selection_image: Option<&Path>,
    ) -> Result<SaveMatchResult> {
        let bytes = std::fs::read(source_image)?;
        let extension = source_image
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .filter(|value| matches!(value.as_str(), "png" | "jpg" | "jpeg" | "bmp"))
            .context("不支持的结算截图格式")?;
        let selection = selection_image
            .map(|path| -> Result<_> {
                let bytes = std::fs::read(path)?;
                let extension = path
                    .extension()
                    .and_then(|value| value.to_str())
                    .map(str::to_ascii_lowercase)
                    .filter(|value| matches!(value.as_str(), "png" | "jpg" | "jpeg" | "bmp"))
                    .context("不支持的局内昵称截图格式")?;
                Ok((bytes, extension))
            })
            .transpose()?;
        self.save_settlement_bytes(
            settlement,
            &bytes,
            &extension,
            annotations,
            selection
                .as_ref()
                .map(|(bytes, extension)| (bytes.as_slice(), extension.as_str())),
        )
    }

    pub fn save_settlement_capture(
        &self,
        settlement: &crate::settlement::SettlementResult,
        source_png: &[u8],
        annotations: &[PlayerAnnotation],
        selection_png: Option<&[u8]>,
    ) -> Result<SaveMatchResult> {
        self.save_settlement_bytes(
            settlement,
            source_png,
            "png",
            annotations,
            selection_png.map(|bytes| (bytes, "png")),
        )
    }

    fn save_settlement_bytes(
        &self,
        settlement: &crate::settlement::SettlementResult,
        bytes: &[u8],
        extension: &str,
        annotations: &[PlayerAnnotation],
        selection_source: Option<(&[u8], &str)>,
    ) -> Result<SaveMatchResult> {
        ensure!(settlement.players.len() == 3, "结算结果必须包含三名玩家");
        ensure!(annotations.len() == 3, "必须提供三名玩家的核对信息");
        for annotation in annotations {
            if let Some(nickname) = annotation.nickname.as_deref() {
                ensure!(nickname.chars().count() <= 80, "昵称长度超出限制");
                ensure!(
                    !nickname.chars().any(char::is_control),
                    "昵称不能包含控制字符"
                );
            }
            ensure!(
                !annotation.name_confirmed
                    || annotation
                        .nickname
                        .as_deref()
                        .is_some_and(|name| !name.trim().is_empty()),
                "确认昵称不能为空"
            );
        }
        ensure!(!bytes.is_empty(), "结算截图为空");
        ensure!(
            matches!(extension, "png" | "jpg" | "jpeg" | "bmp"),
            "不支持的结算截图格式"
        );
        let id = hex_digest(bytes);
        let selection_source = selection_source
            .map(|(bytes, extension)| -> Result<_> {
                ensure!(!bytes.is_empty(), "局内昵称截图为空");
                ensure!(
                    matches!(extension, "png" | "jpg" | "jpeg" | "bmp"),
                    "不支持的局内昵称截图格式"
                );
                Ok((bytes, hex_digest(bytes), extension))
            })
            .transpose()?;

        let connection = Connection::open(&self.database_path)?;
        connection.pragma_update(None, "foreign_keys", true)?;
        let existing: Option<(String, Option<String>)> = connection
            .query_row(
                "SELECT m.saved_at_utc, i.relative_path FROM matches m \
                 LEFT JOIN match_images i ON i.match_id=m.id AND i.kind='settlement' \
                 WHERE m.id=?1",
                [&id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let deduplicated = existing.is_some();
        let saved_at = match &existing {
            Some((saved_at, _)) => saved_at.clone(),
            None => time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)?,
        };
        let relative_path = existing
            .as_ref()
            .and_then(|(_, path)| path.clone())
            .unwrap_or_else(|| format!("evidence/{id}-settlement.{extension}"));

        let mut created_files = Vec::new();
        let prepared_images = (|| -> Result<Vec<(&str, String, String)>> {
            let (image_path, created) = self.prepare_evidence(&relative_path, bytes, &id)?;
            if created {
                created_files.push(image_path);
            }
            let mut images = vec![("settlement", relative_path, id.clone())];
            if let Some((selection_bytes, selection_hash, extension)) = &selection_source {
                let relative = format!(
                    "evidence/{id}-selection-{}.{extension}",
                    &selection_hash[..12]
                );
                let (path, created) =
                    self.prepare_evidence(&relative, selection_bytes, selection_hash)?;
                if created {
                    created_files.push(path);
                }
                images.push(("selection", relative, selection_hash.clone()));
            }
            Ok(images)
        })();
        let images = match prepared_images {
            Ok(images) => images,
            Err(error) => {
                for path in created_files {
                    let _ = std::fs::remove_file(path);
                }
                return Err(error);
            }
        };

        let result = save_match_transaction(
            &connection,
            settlement,
            annotations,
            &id,
            &saved_at,
            &images,
        );
        if result.is_err() {
            for path in created_files {
                let _ = std::fs::remove_file(path);
            }
        }
        result?;
        Ok(SaveMatchResult {
            id,
            saved_at,
            deduplicated,
        })
    }
    pub fn list_matches(&self) -> Result<Vec<SavedMatch>> {
        let connection = Connection::open(&self.database_path)?;
        connection.pragma_update(None, "foreign_keys", true)?;
        let mut statement = connection.prepare(
            "SELECT m.id,m.saved_at_utc,m.rank,m.squad_kills, \
             EXISTS(SELECT 1 FROM match_images i WHERE i.match_id=m.id AND i.kind='settlement'), \
             EXISTS(SELECT 1 FROM match_images i WHERE i.match_id=m.id AND i.kind='selection') \
             FROM matches m ORDER BY m.saved_at_utc DESC,m.id DESC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, bool>(4)?,
                row.get::<_, bool>(5)?,
            ))
        })?;
        let mut matches = Vec::new();
        for row in rows {
            let (id, saved_at, rank, squad_kills, has_settlement_image, has_selection_image) = row?;
            let mut players_statement = connection.prepare(
                "SELECT slot,nickname,name_confirmed,is_local,kills,assists,knocks,damage \
                 FROM players WHERE match_id=?1 ORDER BY slot",
            )?;
            let players = players_statement
                .query_map([&id], |row| {
                    let local: Option<i64> = row.get(3)?;
                    Ok(SavedPlayer {
                        slot: row.get(0)?,
                        nickname: row.get(1)?,
                        name_confirmed: row.get(2)?,
                        is_local: local.map(|value| value != 0),
                        kills: row.get(4)?,
                        assists: row.get(5)?,
                        knocks: row.get(6)?,
                        damage: row.get(7)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            matches.push(SavedMatch {
                id,
                saved_at,
                rank,
                squad_kills,
                players,
                has_settlement_image,
                has_selection_image,
            });
        }
        Ok(matches)
    }

    pub fn evidence_path(&self, match_id: &str, kind: &str) -> Result<PathBuf> {
        ensure!(
            match_id.len() == 64 && match_id.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "对局记录 ID 无效"
        );
        ensure!(matches!(kind, "selection" | "settlement"), "来源图类型无效");
        let connection = Connection::open(&self.database_path)?;
        let relative_path: String = connection.query_row(
            "SELECT relative_path FROM match_images WHERE match_id=?1 AND kind=?2",
            (match_id, kind),
            |row| row.get(0),
        )?;
        let relative = PathBuf::from(relative_path);
        ensure_evidence_relative(&relative)?;
        let evidence_root = self.data_dir.join("evidence").canonicalize()?;
        let path = self.data_dir.join(relative).canonicalize()?;
        ensure!(path.starts_with(evidence_root), "来源图超出数据目录");
        Ok(path)
    }

    fn prepare_evidence(
        &self,
        relative_path: &str,
        bytes: &[u8],
        hash: &str,
    ) -> Result<(PathBuf, bool)> {
        let relative = PathBuf::from(relative_path);
        ensure_evidence_relative(&relative)?;
        let evidence_root = self.data_dir.join("evidence");
        std::fs::create_dir_all(&evidence_root)?;
        let destination = self.data_dir.join(relative);
        if destination.exists() {
            ensure!(
                hex_digest(&std::fs::read(&destination)?) == hash,
                "已有来源图校验失败"
            );
            return Ok((destination, false));
        }
        let temporary = evidence_root.join(format!("{hash}.tmp"));
        std::fs::write(&temporary, bytes)?;
        if let Err(error) = std::fs::rename(&temporary, &destination) {
            let _ = std::fs::remove_file(&temporary);
            if destination.exists() && hex_digest(&std::fs::read(&destination)?) == hash {
                return Ok((destination, false));
            }
            return Err(error.into());
        }
        Ok((destination, true))
    }
}

fn ensure_evidence_relative(relative: &Path) -> Result<()> {
    let mut components = relative.components();
    ensure!(
        matches!(components.next(), Some(Component::Normal(root)) if root == "evidence")
            && components.all(|component| matches!(component, Component::Normal(_))),
        "来源图路径无效"
    );
    Ok(())
}

fn save_match_transaction(
    connection: &Connection,
    settlement: &crate::settlement::SettlementResult,
    annotations: &[PlayerAnnotation],
    id: &str,
    saved_at: &str,
    images: &[(&str, String, String)],
) -> Result<()> {
    let transaction = connection.unchecked_transaction()?;
    transaction.execute(
        "INSERT INTO matches(id,saved_at_utc,rank,squad_kills) VALUES(?1,?2,?3,?4) \
         ON CONFLICT(id) DO UPDATE SET \
           rank=COALESCE(excluded.rank,matches.rank), \
           squad_kills=COALESCE(excluded.squad_kills,matches.squad_kills)",
        (
            id,
            saved_at,
            settlement.rank.value,
            settlement.squad_kills.value,
        ),
    )?;
    for (slot, player) in settlement.players.iter().enumerate() {
        let annotation = &annotations[slot];
        let trusted = annotation.nickname_source != NicknameSource::Unresolved;
        let is_local =
            annotation
                .is_local
                .filter(|_| trusted)
                .map(|value| if value { 1_i64 } else { 0_i64 });
        transaction.execute(
            "INSERT INTO players(match_id,slot,nickname,name_confirmed,is_local,kills,assists,knocks,damage) \
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) \
             ON CONFLICT(match_id,slot) DO UPDATE SET \
               nickname=CASE WHEN excluded.name_confirmed=1 OR players.name_confirmed=0 THEN excluded.nickname ELSE players.nickname END, \
               name_confirmed=MAX(players.name_confirmed,excluded.name_confirmed), \
               is_local=COALESCE(excluded.is_local,players.is_local), \
               kills=COALESCE(excluded.kills,players.kills), \
               assists=COALESCE(excluded.assists,players.assists), \
               knocks=COALESCE(excluded.knocks,players.knocks), \
               damage=COALESCE(excluded.damage,players.damage)",
            (
                id,
                slot as i64,
                annotation.nickname.as_deref().filter(|name| trusted && !name.trim().is_empty()),
                annotation.name_confirmed && trusted,
                is_local,
                player.kills.value,
                player.assists.value,
                player.knocks.value,
                player.damage.value,
            ),
        )?;
    }
    for (kind, relative_path, image_hash) in images {
        transaction.execute(
            "INSERT INTO match_images(match_id,kind,relative_path,sha256) VALUES(?1,?2,?3,?4) \
             ON CONFLICT(match_id,kind) DO UPDATE SET relative_path=excluded.relative_path,sha256=excluded.sha256",
            (id, kind, relative_path, image_hash),
        )?;
    }
    transaction.commit()?;
    Ok(())
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Serialize)]
pub struct LegacyMatch {
    pub id: String,
    pub saved_at: String,
    pub rank: Option<i64>,
    pub squad_kills: Option<i64>,
    pub players: Vec<LegacyPlayer>,
}
#[derive(Serialize)]
pub struct LegacyPlayer {
    pub slot: i64,
    pub nickname: Option<String>,
    pub name_confirmed: bool,
    pub is_local: Option<bool>,
    pub kills: Option<i64>,
    pub assists: Option<i64>,
    pub knocks: Option<i64>,
    pub damage: Option<i64>,
}
#[derive(Serialize)]
pub struct DatabaseReport {
    pub schema: i64,
    pub count: i64,
    pub matches: Vec<LegacyMatch>,
    pub read_only: bool,
}

pub fn inspect(path: &Path) -> Result<DatabaseReport> {
    ensure!(path.is_file(), "请选择旧版 matches.db 文件");
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.pragma_update(None, "query_only", true)?;
    let schema: i64 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
    ensure!(schema == 1, "仅验证旧版数据结构 v1，未修改数据库");
    let count = connection.query_row("SELECT count(*) FROM matches", [], |r| r.get(0))?;
    let mut statement = connection.prepare("SELECT id,saved_at_utc,rank,squad_kills FROM matches ORDER BY saved_at_utc DESC,id DESC LIMIT 50")?;
    let rows = statement.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<i64>>(2)?,
            r.get::<_, Option<i64>>(3)?,
        ))
    })?;
    let mut matches = Vec::new();
    for row in rows {
        let (id, saved_at, rank, squad_kills) = row?;
        let mut players = connection.prepare("SELECT slot,nickname,name_confirmed,is_local,kills,assists,knocks,damage FROM players WHERE match_id=? ORDER BY slot")?;
        let players = players
            .query_map([&id], |r| {
                Ok(LegacyPlayer {
                    slot: r.get(0)?,
                    nickname: r.get(1)?,
                    name_confirmed: r.get(2)?,
                    is_local: r.get(3)?,
                    kills: r.get(4)?,
                    assists: r.get(5)?,
                    knocks: r.get(6)?,
                    damage: r.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        matches.push(LegacyMatch {
            id,
            saved_at,
            rank,
            squad_kills,
            players,
        });
    }
    Ok(DatabaseReport {
        schema,
        count,
        matches,
        read_only: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn number(value: Option<i64>) -> crate::settlement::NumberField {
        crate::settlement::NumberField {
            raw_text: value.map_or_else(String::new, |value| value.to_string()),
            value,
            confidence: 1.0,
        }
    }

    #[test]
    fn legacy_or_unresolved_annotations_never_fall_back_to_summary_hints() -> Result<()> {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../.local/storage-tests/source-guard-{}",
            time::OffsetDateTime::now_utc().unix_timestamp_nanos()
        ));
        let store = LocalStore::open(directory.clone())?;
        let legacy_player = serde_json::json!({"position":"right","nickname":"vjk0ok 0 DANGER","name_confirmed":false,"kills":number(Some(1)),"assists":number(Some(0)),"knocks":number(Some(1)),"damage":number(Some(188))});
        let player: crate::settlement::SettlementPlayer = serde_json::from_value(legacy_player)?;
        assert_eq!(player.nickname_hint.as_deref(), Some("vjk0ok 0 DANGER"));
        let result = crate::settlement::SettlementResult {
            rank: number(Some(8)),
            squad_kills: number(Some(3)),
            players: vec![player.clone(), player.clone(), player],
            warnings: vec![],
        };
        let annotations: Vec<PlayerAnnotation> = (0..3).map(|_|serde_json::from_value(serde_json::json!({"nickname":"vjk0ok 0 DANGER","name_confirmed":true,"is_local":true}))).collect::<std::result::Result<_,_>>()?;
        assert!(annotations
            .iter()
            .all(|a| a.nickname_source == NicknameSource::Unresolved));
        let image = crate::imaging::png(&image::RgbaImage::new(8, 8))?;
        store.save_settlement_capture(&result, &image, &annotations, None)?;
        let records = store.list_matches()?;
        assert!(records[0]
            .players
            .iter()
            .all(|p| p.nickname.is_none() && !p.name_confirmed && p.is_local.is_none()));
        assert_eq!(records[0].players[2].damage, Some(188));
        let trusted: Vec<_> = (0..3)
            .map(|_| PlayerAnnotation {
                nickname: Some("vjkook".into()),
                nickname_source: NicknameSource::VotedRoster,
                name_confirmed: false,
                is_local: None,
            })
            .collect();
        let saved = store.save_settlement_capture(&result, &image, &trusted, None)?;
        assert!(saved.deduplicated);
        assert_eq!(store.list_matches()?.len(), 1);
        assert_eq!(
            store.list_matches()?[0].players[2].nickname.as_deref(),
            Some("vjkook")
        );
        drop(store);
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[test]
    fn new_store_saves_associations_and_deduplicates_capture_images() -> Result<()> {
        let root =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.local/storage-tests");
        std::fs::create_dir_all(&root)?;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let data_dir = root.join(format!("new-store-{nonce}"));
        let selection_source = root.join(format!("selection-{nonce}.jpg"));
        let image_bytes = b"finite storage evidence fixture";
        let selection_bytes = b"finite selection evidence fixture";
        std::fs::write(&selection_source, selection_bytes)?;

        let storage = LocalStore::open(data_dir.clone())?;
        let initial = storage.status()?;
        assert_eq!(initial.record_count, 0);
        assert_eq!(initial.schema_version, NEW_SCHEMA_VERSION);

        let settlement = crate::settlement::SettlementResult {
            rank: number(Some(2)),
            squad_kills: number(Some(0)),
            players: vec![
                crate::settlement::SettlementPlayer {
                    position: "左侧玩家".into(),
                    nickname_hint: Some("队友 A".into()),
                    name_confirmed: false,
                    kills: number(None),
                    assists: number(Some(0)),
                    knocks: number(Some(1)),
                    damage: number(Some(380)),
                },
                crate::settlement::SettlementPlayer {
                    position: "中间玩家".into(),
                    nickname_hint: Some("Archer".into()),
                    name_confirmed: false,
                    kills: number(Some(2)),
                    assists: number(Some(0)),
                    knocks: number(Some(2)),
                    damage: number(Some(480)),
                },
                crate::settlement::SettlementPlayer {
                    position: "右侧玩家".into(),
                    nickname_hint: None,
                    name_confirmed: false,
                    kills: number(Some(0)),
                    assists: number(None),
                    knocks: number(Some(0)),
                    damage: number(Some(60)),
                },
            ],
            warnings: Vec::new(),
        };

        let annotations = vec![
            PlayerAnnotation {
                nickname: Some("已核对玩家".into()),
                nickname_source: NicknameSource::Manual,
                name_confirmed: true,
                is_local: Some(true),
            },
            PlayerAnnotation {
                nickname: Some("Archer".into()),
                nickname_source: NicknameSource::Manual,
                name_confirmed: false,
                is_local: Some(false),
            },
            PlayerAnnotation {
                nickname: None,
                nickname_source: NicknameSource::Manual,
                name_confirmed: false,
                is_local: Some(false),
            },
        ];
        let first = storage.save_settlement_capture(
            &settlement,
            image_bytes,
            &annotations,
            Some(selection_bytes),
        )?;
        assert!(!first.deduplicated);
        let second = storage.save_settlement_capture(
            &settlement,
            image_bytes,
            &annotations,
            Some(selection_bytes),
        )?;
        assert!(second.deduplicated);
        assert_eq!(first.id, second.id);
        assert_eq!(first.saved_at, second.saved_at);

        let records = storage.list_matches()?;
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.rank, Some(2));
        assert_eq!(record.squad_kills, Some(0));
        assert!(record.has_settlement_image);
        assert!(record.has_selection_image);
        assert_eq!(record.players.len(), 3);
        assert_eq!(record.players[0].kills, None);
        assert_eq!(record.players[0].assists, Some(0));
        assert_eq!(record.players[0].is_local, Some(true));
        assert!(record.players[0].name_confirmed);
        assert_eq!(record.players[0].nickname.as_deref(), Some("已核对玩家"));
        assert_eq!(record.players[2].nickname, None);
        let evidence = storage.evidence_path(&record.id, "settlement")?;
        assert_eq!(std::fs::read(evidence)?, image_bytes);
        assert_eq!(
            std::fs::read(storage.evidence_path(&record.id, "selection")?)?,
            selection_bytes
        );

        drop(storage);
        std::fs::remove_file(selection_source)?;
        std::fs::remove_dir_all(data_dir)?;
        Ok(())
    }

    #[test]
    fn saves_annotated_2k_settlement_sample_and_source_image() -> Result<()> {
        let project = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let source = project
            .join("image_for_test/03-游戏结束结算界面-主要用小队排名跟小队总击杀进行定位.jpg");
        let image_bytes = std::fs::read(&source)?;
        let root = project.join(".local/storage-tests");
        std::fs::create_dir_all(&root)?;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let data_dir = root.join(format!("annotated-{nonce}"));
        let storage = LocalStore::open(data_dir.clone())?;
        let settlement = crate::settlement::recognize(&source, &crate::resources()?)?;
        assert_eq!(settlement.rank.value, Some(12));
        assert_eq!(settlement.squad_kills.value, Some(2));
        assert_eq!(
            settlement
                .players
                .iter()
                .map(|player| player.damage.value)
                .collect::<Vec<_>>(),
            vec![Some(174), Some(158), Some(321)]
        );

        let annotations: Vec<_> = settlement
            .players
            .iter()
            .map(|player| PlayerAnnotation {
                nickname: player.nickname_hint.clone(),
                nickname_source: NicknameSource::Unresolved,
                name_confirmed: false,
                is_local: None,
            })
            .collect();
        let first = storage.save_settlement(&settlement, &source, &annotations, None)?;
        let second = storage.save_settlement(&settlement, &source, &annotations, None)?;
        assert!(!first.deduplicated);
        assert!(second.deduplicated);
        assert_eq!(first.id, second.id);
        let records = storage.list_matches()?;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].rank, Some(12));
        assert_eq!(records[0].squad_kills, Some(2));
        assert!(records[0].has_settlement_image);
        assert_eq!(
            std::fs::read(storage.evidence_path(&records[0].id, "settlement")?)?,
            image_bytes
        );

        drop(storage);
        std::fs::remove_dir_all(data_dir)?;
        Ok(())
    }

    #[test]
    fn legacy_null_zero_and_identity_survive_readonly_inspection() -> Result<()> {
        let root =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.local/storage-tests");
        std::fs::create_dir_all(&root)?;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let path = root.join(format!("rust-readonly-{nonce}.db"));
        let connection = Connection::open(&path)?;
        connection.execute_batch("CREATE TABLE matches(id TEXT,saved_at_utc TEXT,rank INTEGER,squad_kills INTEGER); CREATE TABLE players(match_id TEXT,slot INTEGER,nickname TEXT,name_confirmed INTEGER,is_local INTEGER,kills INTEGER,assists INTEGER,knocks INTEGER,damage INTEGER); PRAGMA user_version=1; INSERT INTO matches VALUES('test','2026-10-31T06:11:44+00:00',NULL,0); INSERT INTO players VALUES('test',0,'中文昵称',0,NULL,NULL,0,1,NULL);")?;
        connection.close().map_err(|(_, e)| e)?;
        let before = std::fs::read(&path)?;
        let report = inspect(&path)?;
        assert_eq!(report.count, 1);
        assert_eq!(report.matches[0].rank, None);
        assert_eq!(report.matches[0].squad_kills, Some(0));
        let player = &report.matches[0].players[0];
        assert_eq!(player.kills, None);
        assert_eq!(player.assists, Some(0));
        assert_eq!(player.damage, None);
        assert_eq!(player.is_local, None);
        assert!(!player.name_confirmed);
        assert_eq!(player.nickname.as_deref(), Some("中文昵称"));
        assert_eq!(std::fs::read(&path)?, before);
        std::fs::remove_file(&path)?;
        assert!(inspect(&path).is_err());
        assert!(!path.exists());
        Ok(())
    }
}
