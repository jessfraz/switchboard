mod arguments;
mod gmail;
mod projection;
mod summary;
mod support;

pub(crate) use self::{
    arguments::{
        CliArgsSegment, CliArgsTemplate, CliComputedJsonValue, CliJsonArgumentField, CliJsonArgumentTemplate,
        CliJsonArgumentValue,
    },
    projection::{
        CliJsonEffectSpec, CliJsonFieldMapping, CliJsonProjection, CliJsonProjectionConfig, CliJsonProjectionShape,
        CliJsonRefsSpec, CliProjectionTemplate,
    },
    summary::CliSummaryTemplate,
};

#[cfg(test)]
mod tests;
