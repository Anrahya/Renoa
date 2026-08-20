use rusqlite::{OptionalExtension, params};

use crate::{
    Checkpoint, Command, EffectId, KernelError, OperationId, OperationOutcome, RuntimeManifest,
    SessionId,
    admission::parse_command_id,
    effect_store::parse_effect_id,
    operation_phase::OperationPhase,
    runtime::require_compatible_checkpoint,
    schema::{json_error, sqlite_error},
};

pub(crate) struct StoredOperation {
    pub(crate) command: Command,
    pub(crate) phase: OperationPhase,
    pub(crate) transition_version: i64,
    pub(crate) manifest: Option<RuntimeManifest>,
    pub(crate) checkpoint: Option<Checkpoint>,
    pub(crate) current_effect_id: Option<EffectId>,
    pub(crate) input_effect_id: Option<EffectId>,
    pub(crate) outcome: Option<OperationOutcome>,
}

pub(crate) struct StoredOperationRow {
    command_id: String,
    command_json: String,
    phase: String,
    state_version: i64,
    transition_version: i64,
    manifest_json: Option<String>,
    checkpoint_json: Option<String>,
    current_effect_id: Option<String>,
    input_effect_id: Option<String>,
    outcome_json: Option<String>,
}

impl StoredOperationRow {
    pub(crate) fn read(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<Self> {
        Ok(Self {
            command_id: row.get(offset)?,
            command_json: row.get(offset + 1)?,
            phase: row.get(offset + 2)?,
            state_version: row.get(offset + 3)?,
            transition_version: row.get(offset + 4)?,
            manifest_json: row.get(offset + 5)?,
            checkpoint_json: row.get(offset + 6)?,
            current_effect_id: row.get(offset + 7)?,
            input_effect_id: row.get(offset + 8)?,
            outcome_json: row.get(offset + 9)?,
        })
    }

    pub(crate) fn decode(self) -> Result<StoredOperation, KernelError> {
        require_state_version(self.state_version)?;
        let command: Command = serde_json::from_str(&self.command_json).map_err(json_error)?;
        if command.command_id() != parse_command_id(&self.command_id)? {
            return Err(KernelError::Corrupt(
                "operation command identity differs from stored content".to_owned(),
            ));
        }
        let manifest = self
            .manifest_json
            .map(|value| serde_json::from_str(&value).map_err(json_error))
            .transpose()?;
        let checkpoint = self
            .checkpoint_json
            .map(|value| serde_json::from_str(&value).map_err(json_error))
            .transpose()?;
        if let Some(manifest) = manifest.as_ref() {
            require_compatible_checkpoint(manifest, checkpoint.as_ref())?;
        } else if checkpoint.is_some() {
            return Err(KernelError::Corrupt(
                "operation checkpoint has no runtime manifest".to_owned(),
            ));
        }
        Ok(StoredOperation {
            command,
            phase: OperationPhase::from_database(&self.phase)?,
            transition_version: self.transition_version,
            manifest,
            checkpoint,
            current_effect_id: self
                .current_effect_id
                .map(|value| parse_effect_id(&value))
                .transpose()?,
            input_effect_id: self
                .input_effect_id
                .map(|value| parse_effect_id(&value))
                .transpose()?,
            outcome: self
                .outcome_json
                .map(|value| serde_json::from_str(&value).map_err(json_error))
                .transpose()?,
        })
    }
}

pub(crate) fn load_operation(
    connection: &rusqlite::Connection,
    session_id: SessionId,
    operation_id: OperationId,
) -> Result<Option<StoredOperation>, KernelError> {
    connection
        .query_row(
            "SELECT o.command_id, c.content_json, o.phase, o.state_version,
                    o.transition_version, o.manifest_json, o.checkpoint_json,
                    o.current_effect_id, o.input_effect_id, o.outcome_json
             FROM operations AS o
             JOIN commands AS c
               ON c.session_id = o.session_id AND c.command_id = o.command_id
             WHERE o.session_id = ?1 AND o.operation_id = ?2",
            params![session_id.to_string(), operation_id.to_string()],
            |row| StoredOperationRow::read(row, 0),
        )
        .optional()
        .map_err(sqlite_error)?
        .map(StoredOperationRow::decode)
        .transpose()
}

fn require_state_version(version: i64) -> Result<(), KernelError> {
    let supported = crate::schema::OPERATION_STATE_VERSION;
    match version.cmp(&i64::from(supported)) {
        std::cmp::Ordering::Equal => Ok(()),
        std::cmp::Ordering::Greater => Err(KernelError::UnsupportedStateVersion {
            found: u32::try_from(version).unwrap_or(u32::MAX),
            supported,
        }),
        std::cmp::Ordering::Less => Err(KernelError::Corrupt(format!(
            "invalid operation state version {version}"
        ))),
    }
}
