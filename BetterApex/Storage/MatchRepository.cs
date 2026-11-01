using System.Globalization;
using System.IO;
using System.Security.Cryptography;
using System.Text.Json;
using System.Text.RegularExpressions;
using Microsoft.Data.Sqlite;

namespace BetterApex.Storage;

internal sealed class MatchRepository
{
    private readonly object _gate = new();
    public string DataDirectory { get; }
    public string DatabasePath => Path.Combine(DataDirectory, "matches.db");

    public MatchRepository(string dataDirectory) => DataDirectory = Path.GetFullPath(dataDirectory);

    public static string DefaultDataDirectory()
    {
        if (Environment.GetEnvironmentVariable("BETTER_APEX_DATA_DIR") is { Length: > 0 } configured)
            return Path.GetFullPath(configured);
        for (var directory = new DirectoryInfo(AppContext.BaseDirectory); directory is not null; directory = directory.Parent)
            if (File.Exists(Path.Combine(directory.FullName, "BetterApex.csproj")))
                return Path.Combine(directory.Parent!.FullName, "data");
        return Path.Combine(AppContext.BaseDirectory, "data");
    }

    public SaveMatchResult Save(MatchDraft draft)
    {
        if (draft.CaptureSessionId is not null && !Guid.TryParseExact(draft.CaptureSessionId, "N", out _))
            throw new InvalidDataException("自动采集保存键无效，未保存。");
        var match = draft.Recognition;
        if (match.Players.Count != 3 || (draft.Association is not null
            && (!draft.Association.Players.Select(p => p.Result).SequenceEqual(match.Players) || draft.RosterScreenshot is null)))
            throw new InvalidDataException("结算数据或对应的名单来源不完整，未保存。");
        var players = match.Players.Select((p, slot) =>
        {
            var member = draft.Association?.Players[slot].Member;
            return new StoredPlayer(slot, p.Position, member?.Nickname ?? (draft.Association is null ? p.Name : null),
                member is not null, member is null ? null : member.Position == "本人", p.Kills.Value, p.Assists.Value, p.Knocks.Value, p.Damage.Value);
        }).ToArray();
        if (players.Any(p => new[] { p.Kills, p.Assists, p.Knocks, p.Damage }.Any(v => v < 0)))
            throw new InvalidDataException("战绩包含无效负数，未保存。");
        var payload = JsonSerializer.Serialize(new { Version = 1, match, draft.Association, draft.CaptureSessionId });
        lock (_gate)
        {
            Directory.CreateDirectory(DataDirectory);
            using var connection = Open(create: true);
            EnsureSchema(connection);
            using var transaction = connection.BeginTransaction();
            using var existing = Command(connection, transaction, "SELECT id, payload, (SELECT COUNT(*) FROM players p WHERE p.match_id=m.id AND p.name_confirmed=1) FROM matches m WHERE source_hash=$hash OR id=$session ORDER BY (id=$session) DESC LIMIT 1");
            existing.Parameters.AddWithValue("$hash", draft.Screenshot.Hash);
            existing.Parameters.AddWithValue("$session", Db(draft.CaptureSessionId));
            string? id = null;
            string? oldPayload = null;
            var oldConfirmed = 0;
            using (var reader = existing.ExecuteReader())
                if (reader.Read()) { id = reader.GetString(0); oldPayload = reader.GetString(1); oldConfirmed = reader.GetInt32(2); }
            if (id is not null && oldConfirmed > players.Count(p => p.NameConfirmed))
                return new(id, false, false, "这张截图已有更完整的昵称关联，已保留原记录。");
            var screenshot = WriteImage(draft.Screenshot);
            var rosterImage = draft.RosterScreenshot is null ? null : WriteImage(draft.RosterScreenshot);
            if (id is not null && oldPayload == payload)
                return new(id, false, false, "这张截图已保存，没有重复添加。");

            var created = id is null;
            id ??= draft.CaptureSessionId ?? Guid.NewGuid().ToString("N");
            var now = DateTimeOffset.UtcNow.ToString("O");
            using var command = Command(connection, transaction, """
                INSERT INTO matches(id,source_hash,saved_at_utc,updated_at_utc,recognized_at_utc,rank,squad_kills,screenshot_file,roster_screenshot_file,payload)
                VALUES($id,$hash,$now,$now,$recognized,$rank,$kills,$image,$roster,$payload)
                ON CONFLICT(id) DO UPDATE SET source_hash=$hash,updated_at_utc=$now,recognized_at_utc=$recognized,rank=$rank,squad_kills=$kills,
                screenshot_file=$image,roster_screenshot_file=$roster,payload=$payload;
                """);
            command.Parameters.AddWithValue("$id", id);
            command.Parameters.AddWithValue("$hash", draft.Screenshot.Hash);
            command.Parameters.AddWithValue("$now", now);
            command.Parameters.AddWithValue("$recognized", draft.RecognizedAtUtc.ToUniversalTime().ToString("O"));
            command.Parameters.AddWithValue("$rank", Db(match.Rank.Value));
            command.Parameters.AddWithValue("$kills", Db(match.SquadKills.Value));
            command.Parameters.AddWithValue("$image", screenshot);
            command.Parameters.AddWithValue("$roster", Db(rosterImage));
            command.Parameters.AddWithValue("$payload", payload);
            command.ExecuteNonQuery();
            using var removePlayers = Command(connection, transaction, "DELETE FROM players WHERE match_id=$id");
            removePlayers.Parameters.AddWithValue("$id", id);
            removePlayers.ExecuteNonQuery();
            foreach (var player in players)
            {
                using var insert = Command(connection, transaction, """
                    INSERT INTO players(match_id,slot,position,nickname,name_confirmed,is_local,kills,assists,knocks,damage)
                    VALUES($id,$slot,$position,$name,$confirmed,$local,$kills,$assists,$knocks,$damage)
                    """);
                insert.Parameters.AddWithValue("$id", id);
                insert.Parameters.AddWithValue("$slot", player.Slot);
                insert.Parameters.AddWithValue("$position", player.SourcePosition);
                insert.Parameters.AddWithValue("$name", Db(player.Nickname));
                insert.Parameters.AddWithValue("$confirmed", player.NameConfirmed);
                insert.Parameters.AddWithValue("$local", Db(player.IsLocalPlayer));
                insert.Parameters.AddWithValue("$kills", Db(player.Kills));
                insert.Parameters.AddWithValue("$assists", Db(player.Assists));
                insert.Parameters.AddWithValue("$knocks", Db(player.Knocks));
                insert.Parameters.AddWithValue("$damage", Db(player.Damage));
                insert.ExecuteNonQuery();
            }
            transaction.Commit();
            return new(id, created, !created, created ? "战绩已保存，可在历史记录中查看。" : "已更新同一截图的记录，没有重复添加。");
        }
    }

    public IReadOnlyList<MatchSummary> List()
    {
        lock (_gate)
        {
            if (!File.Exists(DatabasePath)) return [];
            using var connection = Open(create: false);
            ValidateVersion(connection);
            using var command = connection.CreateCommand();
            command.CommandText = """
                SELECT m.id,m.saved_at_utc,m.rank,m.squad_kills,
                  (SELECT group_concat(COALESCE(NULLIF(nickname,''),'待核对'),' / ') FROM
                    (SELECT nickname FROM players WHERE match_id=m.id ORDER BY slot))
                FROM matches m ORDER BY m.saved_at_utc DESC,m.id DESC
                """;
            using var reader = command.ExecuteReader();
            var result = new List<MatchSummary>();
            while (reader.Read())
                result.Add(new(reader.GetString(0), ParseDate(reader.GetString(1)), Int(reader, 2), Int(reader, 3), reader.IsDBNull(4) ? "待核对" : reader.GetString(4)));
            return result;
        }
    }

    public StoredMatch? Get(string id)
    {
        lock (_gate)
        {
            if (!File.Exists(DatabasePath)) return null;
            using var connection = Open(create: false);
            ValidateVersion(connection);
            using var command = connection.CreateCommand();
            command.CommandText = "SELECT id,saved_at_utc,updated_at_utc,recognized_at_utc,rank,squad_kills,screenshot_file,roster_screenshot_file FROM matches WHERE id=$id";
            command.Parameters.AddWithValue("$id", id);
            StoredMatch match;
            using (var reader = command.ExecuteReader())
            {
                if (!reader.Read()) return null;
                match = new(reader.GetString(0), ParseDate(reader.GetString(1)), ParseDate(reader.GetString(2)), ParseDate(reader.GetString(3)),
                    Int(reader, 4), Int(reader, 5), reader.GetString(6), reader.IsDBNull(7) ? null : reader.GetString(7), []);
            }
            command.CommandText = "SELECT slot,position,nickname,name_confirmed,is_local,kills,assists,knocks,damage FROM players WHERE match_id=$id ORDER BY slot";
            using var rows = command.ExecuteReader();
            var players = new List<StoredPlayer>();
            while (rows.Read())
                players.Add(new(rows.GetInt32(0), rows.GetString(1), rows.IsDBNull(2) ? null : rows.GetString(2), rows.GetBoolean(3),
                    rows.IsDBNull(4) ? null : rows.GetBoolean(4), Int(rows, 5), Int(rows, 6), Int(rows, 7), Int(rows, 8)));
            if (players.Count != 3) throw new InvalidDataException("这条记录的数据不完整，未能显示详情。");
            return match with { Players = players };
        }
    }

    public string GetImagePath(string relative)
    {
        if (!Regex.IsMatch(relative, @"^screenshots/[A-F0-9]{64}\.(png|jpg|jpeg|bmp)$"))
            throw new InvalidDataException("保存的截图路径无效。");
        return Path.Combine(DataDirectory, relative.Replace('/', Path.DirectorySeparatorChar));
    }

    public string GetVerifiedImagePath(string relative)
    {
        var path = GetImagePath(relative);
        VerifyImage(path, Path.GetFileNameWithoutExtension(path));
        return path;
    }

    private string WriteImage(ImageEvidence evidence)
    {
        if (Convert.ToHexString(SHA256.HashData(evidence.Bytes.Span)) != evidence.Hash)
            throw new InvalidDataException("来源截图校验失败，未保存。");
        var relative = $"screenshots/{evidence.Hash}{evidence.Extension}";
        var target = GetImagePath(relative);
        Directory.CreateDirectory(Path.GetDirectoryName(target)!);
        if (File.Exists(target)) { VerifyImage(target, evidence.Hash); return relative; }
        var temporary = target + "." + Guid.NewGuid().ToString("N") + ".tmp";
        try
        {
            using (var stream = new FileStream(temporary, FileMode.CreateNew, FileAccess.Write, FileShare.None))
            { stream.Write(evidence.Bytes.Span); stream.Flush(flushToDisk: true); }
            try { File.Move(temporary, target); }
            catch (IOException) when (File.Exists(target)) { VerifyImage(target, evidence.Hash); }
        }
        finally { if (File.Exists(temporary)) File.Delete(temporary); }
        return relative;
    }

    private static void VerifyImage(string path, string hash)
    {
        using var stream = File.OpenRead(path);
        if (Convert.ToHexString(SHA256.HashData(stream)) != hash)
            throw new InvalidDataException("本地保存的来源截图已损坏，请检查数据目录；已有战绩未被覆盖。");
    }

    private SqliteConnection Open(bool create)
    {
        var connection = new SqliteConnection(new SqliteConnectionStringBuilder
        {
            DataSource = DatabasePath, Mode = create ? SqliteOpenMode.ReadWriteCreate : SqliteOpenMode.ReadWrite,
            ForeignKeys = true, Pooling = false, DefaultTimeout = 5
        }.ToString());
        try { connection.Open(); return connection; }
        catch { connection.Dispose(); throw; }
    }

    private static void EnsureSchema(SqliteConnection connection)
    {
        using var transaction = connection.BeginTransaction();
        using var version = Command(connection, transaction, "PRAGMA user_version");
        var current = Convert.ToInt32(version.ExecuteScalar());
        if (current == 1) { transaction.Commit(); return; }
        if (current != 0) throw new InvalidDataException("本地数据版本不受当前程序支持，未修改已有数据。");
        using var command = Command(connection, transaction, """
            CREATE TABLE matches(
              id TEXT PRIMARY KEY,source_hash TEXT NOT NULL UNIQUE,saved_at_utc TEXT NOT NULL,updated_at_utc TEXT NOT NULL,
              recognized_at_utc TEXT NOT NULL,rank INTEGER,squad_kills INTEGER,screenshot_file TEXT NOT NULL,roster_screenshot_file TEXT,payload TEXT NOT NULL);
            CREATE TABLE players(
              match_id TEXT NOT NULL REFERENCES matches(id) ON DELETE CASCADE,slot INTEGER NOT NULL CHECK(slot BETWEEN 0 AND 2),
              position TEXT NOT NULL,nickname TEXT,name_confirmed INTEGER NOT NULL,is_local INTEGER,
              kills INTEGER,assists INTEGER,knocks INTEGER,damage INTEGER,PRIMARY KEY(match_id,slot));
            PRAGMA user_version=1;
            """);
        command.ExecuteNonQuery();
        transaction.Commit();
    }

    private static void ValidateVersion(SqliteConnection connection)
    {
        using var command = connection.CreateCommand();
        command.CommandText = "PRAGMA user_version";
        if (Convert.ToInt32(command.ExecuteScalar()) != 1)
            throw new InvalidDataException("本地数据版本不受当前程序支持，未修改已有数据。");
    }

    private static SqliteCommand Command(SqliteConnection connection, SqliteTransaction transaction, string sql)
    { var command = connection.CreateCommand(); command.Transaction = transaction; command.CommandText = sql; return command; }
    private static object Db(object? value) => value ?? DBNull.Value;
    private static int? Int(SqliteDataReader reader, int ordinal) => reader.IsDBNull(ordinal) ? null : reader.GetInt32(ordinal);
    private static DateTimeOffset ParseDate(string value) => DateTimeOffset.Parse(value, CultureInfo.InvariantCulture, DateTimeStyles.RoundtripKind);
}
