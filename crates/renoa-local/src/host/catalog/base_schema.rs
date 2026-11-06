//! The tables a new catalog starts from, before the steps that bring it to
//! the current schema; see `initialize_connection`.

pub(super) const SCHEMA: &str = "
    CREATE TABLE host_metadata (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
        schema_version INTEGER NOT NULL CHECK (schema_version > 0)
    ) STRICT;

    CREATE TABLE mcp_integrations (
        integration_id TEXT PRIMARY KEY CHECK (length(integration_id) > 0),
        kind TEXT NOT NULL CHECK (kind = 'direct_streamable_http'),
        endpoint TEXT NOT NULL CHECK (length(endpoint) > 0),
        request_headers_json TEXT NOT NULL CHECK (
            json_valid(request_headers_json)
            AND json_type(request_headers_json) = 'object'
        )
    ) STRICT;

    CREATE TABLE mcp_connections (
        connection_id TEXT PRIMARY KEY CHECK (length(connection_id) > 0),
        integration_id TEXT NOT NULL REFERENCES mcp_integrations(integration_id),
        auth_kind TEXT NOT NULL CHECK (
            auth_kind IN (
                'none', 'gh_cli', 'secret_service_bearer',
                'secret_service_header', 'oauth'
            )
        ),
        auth_hostname TEXT,
        auth_account TEXT,
        auth_credential_id TEXT,
        oauth_registration_json TEXT,
        auth_header_name TEXT,
        auth_header_prefix TEXT,
        CHECK (
            (auth_kind = 'none' AND auth_hostname IS NULL AND auth_account IS NULL
             AND auth_credential_id IS NULL AND oauth_registration_json IS NULL
             AND auth_header_name IS NULL AND auth_header_prefix IS NULL)
            OR
            (auth_kind = 'gh_cli'
             AND length(auth_hostname) > 0
             AND length(auth_account) > 0
             AND auth_credential_id IS NULL AND oauth_registration_json IS NULL
             AND auth_header_name IS NULL AND auth_header_prefix IS NULL)
            OR
            (auth_kind = 'secret_service_bearer'
             AND auth_hostname IS NULL
             AND auth_account IS NULL
             AND length(auth_credential_id) > 0
             AND oauth_registration_json IS NULL
             AND auth_header_name IS NULL AND auth_header_prefix IS NULL)
            OR
            (auth_kind = 'secret_service_header'
             AND auth_hostname IS NULL
             AND auth_account IS NULL
             AND length(auth_credential_id) > 0
             AND oauth_registration_json IS NULL
             AND length(auth_header_name) > 0
             AND auth_header_prefix IS NOT NULL)
            OR
            (auth_kind = 'oauth'
             AND auth_hostname IS NULL
             AND auth_account IS NULL
             AND length(auth_credential_id) > 0
             AND json_valid(oauth_registration_json)
             AND json_type(oauth_registration_json) = 'object'
             AND auth_header_name IS NULL AND auth_header_prefix IS NULL)
        )
    ) STRICT;

    CREATE TABLE mcp_catalogs (
        connection_id TEXT PRIMARY KEY
            REFERENCES mcp_connections(connection_id) ON DELETE CASCADE,
        endpoint TEXT NOT NULL CHECK (length(endpoint) > 0),
        request_headers_json TEXT NOT NULL CHECK (
            json_valid(request_headers_json)
            AND json_type(request_headers_json) = 'object'
        ),
        protocol_version TEXT NOT NULL CHECK (length(protocol_version) > 0),
        adapter_revision TEXT NOT NULL CHECK (length(adapter_revision) > 0),
        catalog_digest TEXT NOT NULL CHECK (length(catalog_digest) = 64)
    ) STRICT;

    CREATE TABLE mcp_tools (
        connection_id TEXT NOT NULL
            REFERENCES mcp_catalogs(connection_id) ON DELETE CASCADE,
        name TEXT NOT NULL CHECK (length(name) > 0),
        description TEXT NOT NULL,
        input_schema_json TEXT NOT NULL CHECK (json_valid(input_schema_json)),
        model_input_schema_json TEXT NOT NULL CHECK (json_valid(model_input_schema_json)),
        output_schema_json TEXT CHECK (
            output_schema_json IS NULL OR json_valid(output_schema_json)
        ),
        PRIMARY KEY (connection_id, name)
    ) STRICT;

    CREATE TABLE mcp_rejected_tools (
        connection_id TEXT NOT NULL
            REFERENCES mcp_catalogs(connection_id) ON DELETE CASCADE,
        source_index INTEGER NOT NULL CHECK (source_index >= 0),
        name TEXT,
        reason TEXT NOT NULL,
        PRIMARY KEY (connection_id, source_index)
    ) STRICT;

    CREATE TABLE mcp_oauth_flows (
        connection_id TEXT PRIMARY KEY CHECK (length(connection_id) > 0),
        operation_id TEXT NOT NULL CHECK (length(operation_id) BETWEEN 1 AND 512),
        phase TEXT NOT NULL CHECK (
            phase IN (
                'begin_in_flight', 'awaiting_callback', 'callback_ready',
                'exchange_in_flight', 'refresh_in_flight', 'unknown'
            )
        ),
        callback_port INTEGER,
        callback_relay_id TEXT,
        expires_at_ms INTEGER,
        CHECK (
            (phase IN ('begin_in_flight', 'awaiting_callback', 'callback_ready',
                       'exchange_in_flight')
             AND ((callback_port BETWEEN 1 AND 65535 AND callback_relay_id IS NULL)
                  OR (callback_port IS NULL AND length(callback_relay_id) = 36))
             AND expires_at_ms > 0)
            OR
            (phase IN ('refresh_in_flight', 'unknown')
             AND callback_port IS NULL
             AND callback_relay_id IS NULL
             AND expires_at_ms IS NULL)
        )
    ) STRICT;

    CREATE TABLE mcp_oauth_receipts (
        connection_id TEXT NOT NULL CHECK (length(connection_id) > 0),
        operation_id TEXT NOT NULL CHECK (length(operation_id) BETWEEN 1 AND 512),
        outcome_json TEXT NOT NULL CHECK (
            length(outcome_json) BETWEEN 1 AND 16384
            AND json_valid(outcome_json)
            AND json_type(outcome_json) = 'object'
        ),
        PRIMARY KEY (connection_id, operation_id)
    ) STRICT;

    CREATE TABLE skill_revisions (
        skill_digest TEXT PRIMARY KEY CHECK (length(skill_digest) = 64),
        name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 64),
        description TEXT NOT NULL CHECK (length(description) BETWEEN 1 AND 1024),
        license TEXT,
        compatibility TEXT,
        UNIQUE (skill_digest, name)
    ) STRICT;

    CREATE TABLE installed_plugins (
        plugin_digest TEXT PRIMARY KEY CHECK (length(plugin_digest) = 64),
        name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 64),
        version TEXT,
        description TEXT,
        homepage TEXT,
        repository TEXT,
        license TEXT
    ) STRICT;

    CREATE TABLE plugin_mcp_servers (
        plugin_digest TEXT NOT NULL
            REFERENCES installed_plugins(plugin_digest) ON DELETE RESTRICT,
        server_id TEXT NOT NULL CHECK (length(server_id) BETWEEN 1 AND 128),
        integration_id TEXT NOT NULL UNIQUE,
        transport TEXT NOT NULL CHECK (transport = 'streamable_http'),
        endpoint TEXT NOT NULL CHECK (length(endpoint) > 0),
        request_headers_json TEXT NOT NULL CHECK (
            json_valid(request_headers_json)
            AND json_type(request_headers_json) = 'object'
        ),
        PRIMARY KEY (plugin_digest, server_id)
    ) STRICT;

    CREATE TABLE shared_plugin_registry_state (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
        registry_id TEXT NOT NULL CHECK (length(registry_id) = 36),
        applied_revision INTEGER NOT NULL CHECK (applied_revision >= 0)
    ) STRICT;

    INSERT INTO host_metadata(singleton, schema_version) VALUES (1, 13);
";
