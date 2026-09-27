-- Discord schema v1 from source commit 9ca328b.
CREATE TABLE identity (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            guild_id TEXT NOT NULL CHECK (
                length(guild_id) BETWEEN 1 AND 20
                AND guild_id NOT GLOB '*[^0-9]*'
                AND guild_id NOT GLOB '0*'
            ),
            operator_user_id TEXT NOT NULL CHECK (
                length(operator_user_id) BETWEEN 1 AND 20
                AND operator_user_id NOT GLOB '*[^0-9]*'
                AND operator_user_id NOT GLOB '0*'
            ),
            agent_id TEXT NOT NULL CHECK (length(agent_id) = 36),
            bot_user_id TEXT CHECK (
                bot_user_id IS NULL
                OR (
                    length(bot_user_id) BETWEEN 1 AND 20
                    AND bot_user_id NOT GLOB '*[^0-9]*'
                    AND bot_user_id NOT GLOB '0*'
                )
            )
         ) STRICT;

         CREATE TABLE messages (
            message_id TEXT PRIMARY KEY CHECK (
                length(message_id) BETWEEN 1 AND 20
                AND message_id NOT GLOB '*[^0-9]*'
                AND message_id NOT GLOB '0*'
            ),
            channel_id TEXT NOT NULL CHECK (
                length(channel_id) BETWEEN 1 AND 20
                AND channel_id NOT GLOB '*[^0-9]*'
                AND channel_id NOT GLOB '0*'
            ),
            author_id TEXT NOT NULL CHECK (
                length(author_id) BETWEEN 1 AND 20
                AND author_id NOT GLOB '*[^0-9]*'
                AND author_id NOT GLOB '0*'
            ),
            canonical BLOB NOT NULL CHECK (length(canonical) > 0),
            created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0)
         ) STRICT;

         
CREATE TABLE conversations (
    channel_id TEXT PRIMARY KEY CHECK (
        length(channel_id) BETWEEN 1 AND 20
        AND channel_id NOT GLOB '*[^0-9]*'
        AND channel_id NOT GLOB '0*'
    ),
    session_id TEXT NOT NULL CHECK (length(session_id) = 36)
) STRICT;

CREATE TABLE turns (
    message_id TEXT PRIMARY KEY REFERENCES messages(message_id),
    session_id TEXT NOT NULL CHECK (length(session_id) = 36),
    request_id TEXT NOT NULL CHECK (length(request_id) = 36),
    prompt TEXT NOT NULL,
    result TEXT,
    state TEXT NOT NULL CHECK (state IN ('queued', 'running', 'ready')),
    CHECK (
        (state IN ('queued', 'running') AND result IS NULL)
        OR (state = 'ready' AND result IS NOT NULL)
    )
) STRICT;

CREATE TABLE deliveries (
    message_id TEXT NOT NULL REFERENCES turns(message_id),
    chunk INTEGER NOT NULL CHECK (chunk >= 0),
    body TEXT NOT NULL CHECK (length(body) > 0),
    state TEXT NOT NULL CHECK (state IN ('pending', 'sending', 'sent', 'unknown', 'failed')),
    reply_id TEXT,
    PRIMARY KEY (message_id, chunk),
    CHECK (
        (state = 'sent' AND reply_id IS NOT NULL)
        OR (state <> 'sent' AND reply_id IS NULL)
    )
) STRICT;

CREATE TABLE gateway (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    session_id TEXT,
    resume_url TEXT,
    sequence INTEGER,
    CHECK (
        (session_id IS NULL AND resume_url IS NULL)
        OR (length(session_id) > 0 AND length(resume_url) > 0)
    )
) STRICT;

CREATE INDEX turns_ready ON turns(message_id) WHERE state = 'ready';
CREATE INDEX turns_queued ON turns(message_id) WHERE state = 'queued';

PRAGMA user_version = 1;
