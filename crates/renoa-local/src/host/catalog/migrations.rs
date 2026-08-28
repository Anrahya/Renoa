pub(super) const MIGRATE_V1_TO_V2: &str = "
    CREATE TABLE mcp_connections_v2 (
        connection_id TEXT PRIMARY KEY CHECK (length(connection_id) > 0),
        integration_id TEXT NOT NULL REFERENCES mcp_integrations(integration_id),
        auth_kind TEXT NOT NULL CHECK (auth_kind IN ('none', 'gh_cli')),
        auth_hostname TEXT,
        auth_account TEXT,
        CHECK (
            (auth_kind = 'none' AND auth_hostname IS NULL AND auth_account IS NULL)
            OR
            (auth_kind = 'gh_cli'
             AND length(auth_hostname) > 0
             AND length(auth_account) > 0)
        )
    ) STRICT;

    INSERT INTO mcp_connections_v2(
        connection_id, integration_id, auth_kind, auth_hostname, auth_account
    )
    SELECT connection_id, integration_id, auth_kind, NULL, NULL
    FROM mcp_connections;

    DROP TABLE mcp_connections;
    ALTER TABLE mcp_connections_v2 RENAME TO mcp_connections;
    UPDATE host_metadata SET schema_version = 2 WHERE singleton = 1;
";

pub(super) const MIGRATE_V2_TO_V3: &str = "
    CREATE TABLE profile_mcp_connections (
        profile_id TEXT NOT NULL CHECK (length(profile_id) > 0),
        connection_id TEXT NOT NULL
            REFERENCES mcp_connections(connection_id) ON DELETE RESTRICT,
        PRIMARY KEY (profile_id, connection_id)
    ) STRICT;

    INSERT INTO profile_mcp_connections(profile_id, connection_id)
    SELECT DISTINCT profile_id, connection_id FROM profile_mcp_tools;

    DROP TABLE profile_mcp_tools;
    UPDATE host_metadata SET schema_version = 3 WHERE singleton = 1;
";

pub(super) const MIGRATE_V3_TO_V4: &str = "
    CREATE TABLE skill_revisions (
        skill_digest TEXT PRIMARY KEY CHECK (length(skill_digest) = 64),
        name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 64),
        description TEXT NOT NULL CHECK (length(description) BETWEEN 1 AND 1024),
        license TEXT,
        compatibility TEXT,
        UNIQUE (skill_digest, name)
    ) STRICT;

    CREATE TABLE profile_skill_bindings (
        profile_id TEXT NOT NULL CHECK (length(profile_id) > 0),
        scope_kind TEXT NOT NULL CHECK (scope_kind IN ('global', 'workspace')),
        workspace TEXT,
        source_root TEXT NOT NULL CHECK (length(source_root) > 0),
        skill_name TEXT NOT NULL CHECK (length(skill_name) > 0),
        skill_digest TEXT NOT NULL,
        FOREIGN KEY (skill_digest, skill_name)
            REFERENCES skill_revisions(skill_digest, name) ON DELETE RESTRICT,
        CHECK (
            (scope_kind = 'global' AND workspace IS NULL)
            OR
            (scope_kind = 'workspace' AND length(workspace) > 0)
        ),
        PRIMARY KEY (profile_id, source_root, skill_name)
    ) STRICT;

    CREATE TABLE skill_source_rejections (
        profile_id TEXT NOT NULL CHECK (length(profile_id) > 0),
        scope_kind TEXT NOT NULL CHECK (scope_kind IN ('global', 'workspace')),
        workspace TEXT,
        source_root TEXT NOT NULL CHECK (length(source_root) > 0),
        entry_name TEXT NOT NULL CHECK (length(entry_name) > 0),
        reason TEXT NOT NULL CHECK (length(reason) > 0),
        CHECK (
            (scope_kind = 'global' AND workspace IS NULL)
            OR
            (scope_kind = 'workspace' AND length(workspace) > 0)
        ),
        PRIMARY KEY (profile_id, source_root, entry_name)
    ) STRICT;

    CREATE TABLE session_skills (
        activation_order INTEGER PRIMARY KEY AUTOINCREMENT,
        session_id TEXT NOT NULL CHECK (length(session_id) > 0),
        activation_command_id TEXT NOT NULL CHECK (length(activation_command_id) > 0),
        skill_name TEXT NOT NULL CHECK (length(skill_name) > 0),
        skill_digest TEXT NOT NULL,
        instruction_bytes INTEGER NOT NULL CHECK (instruction_bytes > 0),
        FOREIGN KEY (skill_digest, skill_name)
            REFERENCES skill_revisions(skill_digest, name) ON DELETE RESTRICT,
        UNIQUE (session_id, skill_name),
        UNIQUE (session_id, skill_digest)
    ) STRICT;

    UPDATE host_metadata SET schema_version = 4 WHERE singleton = 1;
";

pub(super) const MIGRATE_V4_TO_V5: &str = "
    CREATE TABLE session_skills_v5 (
        activation_order INTEGER PRIMARY KEY AUTOINCREMENT,
        session_id TEXT NOT NULL CHECK (length(session_id) > 0),
        activation_command_id TEXT NOT NULL CHECK (length(activation_command_id) > 0),
        skill_name TEXT NOT NULL CHECK (length(skill_name) > 0),
        skill_digest TEXT NOT NULL,
        FOREIGN KEY (skill_digest, skill_name)
            REFERENCES skill_revisions(skill_digest, name) ON DELETE RESTRICT,
        UNIQUE (session_id, skill_name),
        UNIQUE (session_id, skill_digest)
    ) STRICT;

    INSERT INTO session_skills_v5(
        activation_order, session_id, activation_command_id, skill_name, skill_digest
    )
    SELECT activation_order, session_id, activation_command_id, skill_name, skill_digest
    FROM session_skills
    ORDER BY activation_order;

    DROP TABLE session_skills;
    ALTER TABLE session_skills_v5 RENAME TO session_skills;
    UPDATE host_metadata SET schema_version = 5 WHERE singleton = 1;
";

pub(super) const MIGRATE_V5_TO_V6: &str = "
    ALTER TABLE mcp_integrations ADD COLUMN request_headers_json TEXT NOT NULL
        DEFAULT '{}' CHECK (
            json_valid(request_headers_json)
            AND json_type(request_headers_json) = 'object'
        );

    CREATE TABLE mcp_connections_v6 (
        connection_id TEXT PRIMARY KEY CHECK (length(connection_id) > 0),
        integration_id TEXT NOT NULL REFERENCES mcp_integrations(integration_id),
        auth_kind TEXT NOT NULL CHECK (
            auth_kind IN ('none', 'gh_cli', 'secret_service_bearer')
        ),
        auth_hostname TEXT,
        auth_account TEXT,
        auth_credential_id TEXT,
        CHECK (
            (auth_kind = 'none' AND auth_hostname IS NULL AND auth_account IS NULL
             AND auth_credential_id IS NULL)
            OR
            (auth_kind = 'gh_cli'
             AND length(auth_hostname) > 0
             AND length(auth_account) > 0
             AND auth_credential_id IS NULL)
            OR
            (auth_kind = 'secret_service_bearer'
             AND auth_hostname IS NULL
             AND auth_account IS NULL
             AND length(auth_credential_id) > 0)
        )
    ) STRICT;

    INSERT INTO mcp_connections_v6(
        connection_id, integration_id, auth_kind, auth_hostname, auth_account,
        auth_credential_id
    )
    SELECT connection_id, integration_id, auth_kind, auth_hostname, auth_account, NULL
    FROM mcp_connections;

    DROP TABLE mcp_connections;
    ALTER TABLE mcp_connections_v6 RENAME TO mcp_connections;

    ALTER TABLE mcp_catalogs ADD COLUMN request_headers_json TEXT NOT NULL
        DEFAULT '{}' CHECK (
            json_valid(request_headers_json)
            AND json_type(request_headers_json) = 'object'
        );

    CREATE TABLE installed_plugins (
        plugin_digest TEXT PRIMARY KEY CHECK (length(plugin_digest) = 64),
        name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 64),
        version TEXT,
        description TEXT,
        repository TEXT,
        license TEXT
    ) STRICT;

    CREATE TABLE plugin_mcp_servers (
        plugin_digest TEXT NOT NULL
            REFERENCES installed_plugins(plugin_digest) ON DELETE RESTRICT,
        server_id TEXT NOT NULL CHECK (length(server_id) BETWEEN 1 AND 128),
        transport TEXT NOT NULL CHECK (transport = 'streamable_http'),
        endpoint TEXT NOT NULL CHECK (length(endpoint) > 0),
        request_headers_json TEXT NOT NULL CHECK (
            json_valid(request_headers_json)
            AND json_type(request_headers_json) = 'object'
        ),
        PRIMARY KEY (plugin_digest, server_id)
    ) STRICT;

    UPDATE host_metadata SET schema_version = 6 WHERE singleton = 1;
";

pub(super) const MIGRATE_V6_TO_V7: &str = "
    ALTER TABLE installed_plugins ADD COLUMN homepage TEXT;

    CREATE TABLE profile_skill_bindings_v7 (
        profile_id TEXT NOT NULL CHECK (length(profile_id) > 0),
        scope_kind TEXT NOT NULL CHECK (
            scope_kind IN ('global', 'workspace', 'plugin')
        ),
        workspace TEXT,
        source_id TEXT NOT NULL CHECK (length(source_id) > 0),
        skill_name TEXT NOT NULL CHECK (length(skill_name) > 0),
        skill_digest TEXT NOT NULL,
        FOREIGN KEY (skill_digest, skill_name)
            REFERENCES skill_revisions(skill_digest, name) ON DELETE RESTRICT,
        CHECK (
            (scope_kind IN ('global', 'plugin') AND workspace IS NULL)
            OR
            (scope_kind = 'workspace' AND length(workspace) > 0)
        ),
        PRIMARY KEY (profile_id, source_id, skill_name)
    ) STRICT;

    INSERT INTO profile_skill_bindings_v7(
        profile_id, scope_kind, workspace, source_id, skill_name, skill_digest
    )
    SELECT profile_id, scope_kind, workspace, source_root, skill_name, skill_digest
    FROM profile_skill_bindings;

    DROP TABLE profile_skill_bindings;
    ALTER TABLE profile_skill_bindings_v7 RENAME TO profile_skill_bindings;

    CREATE TABLE skill_source_rejections_v7 (
        profile_id TEXT NOT NULL CHECK (length(profile_id) > 0),
        scope_kind TEXT NOT NULL CHECK (
            scope_kind IN ('global', 'workspace', 'plugin')
        ),
        workspace TEXT,
        source_id TEXT NOT NULL CHECK (length(source_id) > 0),
        entry_name TEXT NOT NULL CHECK (length(entry_name) > 0),
        reason TEXT NOT NULL CHECK (length(reason) > 0),
        CHECK (
            (scope_kind IN ('global', 'plugin') AND workspace IS NULL)
            OR
            (scope_kind = 'workspace' AND length(workspace) > 0)
        ),
        PRIMARY KEY (profile_id, source_id, entry_name)
    ) STRICT;

    INSERT INTO skill_source_rejections_v7(
        profile_id, scope_kind, workspace, source_id, entry_name, reason
    )
    SELECT profile_id, scope_kind, workspace, source_root, entry_name, reason
    FROM skill_source_rejections;

    DROP TABLE skill_source_rejections;
    ALTER TABLE skill_source_rejections_v7 RENAME TO skill_source_rejections;

    UPDATE host_metadata SET schema_version = 7 WHERE singleton = 1;
";

pub(super) const MIGRATE_V7_TO_V8: &str = "
    CREATE TABLE mcp_connections_v8 (
        connection_id TEXT PRIMARY KEY CHECK (length(connection_id) > 0),
        integration_id TEXT NOT NULL REFERENCES mcp_integrations(integration_id),
        auth_kind TEXT NOT NULL CHECK (
            auth_kind IN ('none', 'gh_cli', 'secret_service_bearer', 'oauth')
        ),
        auth_hostname TEXT,
        auth_account TEXT,
        auth_credential_id TEXT,
        CHECK (
            (auth_kind = 'none' AND auth_hostname IS NULL AND auth_account IS NULL
             AND auth_credential_id IS NULL)
            OR
            (auth_kind = 'gh_cli'
             AND length(auth_hostname) > 0
             AND length(auth_account) > 0
             AND auth_credential_id IS NULL)
            OR
            (auth_kind IN ('secret_service_bearer', 'oauth')
             AND auth_hostname IS NULL
             AND auth_account IS NULL
             AND length(auth_credential_id) > 0)
        )
    ) STRICT;

    INSERT INTO mcp_connections_v8(
        connection_id, integration_id, auth_kind, auth_hostname, auth_account,
        auth_credential_id
    )
    SELECT connection_id, integration_id, auth_kind, auth_hostname, auth_account,
           auth_credential_id
    FROM mcp_connections;

    DROP TABLE mcp_connections;
    ALTER TABLE mcp_connections_v8 RENAME TO mcp_connections;

    CREATE TABLE mcp_oauth_flows (
        connection_id TEXT PRIMARY KEY
            REFERENCES mcp_connections(connection_id) ON DELETE CASCADE,
        operation_id TEXT NOT NULL CHECK (length(operation_id) BETWEEN 1 AND 512),
        phase TEXT NOT NULL CHECK (
            phase IN (
                'begin_in_flight', 'awaiting_callback', 'callback_ready',
                'exchange_in_flight', 'refresh_in_flight', 'unknown'
            )
        ),
        callback_port INTEGER,
        expires_at_ms INTEGER,
        CHECK (
            (phase IN ('begin_in_flight', 'awaiting_callback', 'callback_ready',
                       'exchange_in_flight')
             AND callback_port BETWEEN 1 AND 65535
             AND expires_at_ms > 0)
            OR
            (phase IN ('refresh_in_flight', 'unknown')
             AND callback_port IS NULL
             AND expires_at_ms IS NULL)
        )
    ) STRICT;

    CREATE TABLE mcp_oauth_receipts (
        connection_id TEXT NOT NULL
            REFERENCES mcp_connections(connection_id) ON DELETE CASCADE,
        operation_id TEXT NOT NULL CHECK (length(operation_id) BETWEEN 1 AND 512),
        outcome_json TEXT NOT NULL CHECK (
            length(outcome_json) BETWEEN 1 AND 16384
            AND json_valid(outcome_json)
            AND json_type(outcome_json) = 'object'
        ),
        PRIMARY KEY (connection_id, operation_id)
    ) STRICT;

    UPDATE host_metadata SET schema_version = 8 WHERE singleton = 1;
";
