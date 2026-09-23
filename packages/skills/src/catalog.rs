use crate::{error, SkillRegistry, SKILLS_SERVICE};
use api::{
    async_trait, ContextBlock, ContextTransform, ErrorCode, Message, Plugin, PluginManifest, Registrar, Result,
    RunContext, ServiceRegistration,
};
use std::sync::Arc;

pub struct SkillCatalogTransform {
    registry: Arc<SkillRegistry>,
}
impl SkillCatalogTransform {
    pub fn new(registry: Arc<SkillRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl ContextTransform for SkillCatalogTransform {
    async fn transform(&self, _ctx: &RunContext, messages: Vec<Message>) -> Result<Vec<Message>> {
        Ok(messages)
    }
    async fn sources(
        &self,
        ctx: &RunContext,
        _messages: &[Message],
    ) -> Result<Vec<ContextBlock>> {
        if !ctx.tools_enabled
            || ctx
                .allowed_tools
                .as_ref()
                .is_some_and(|tools| !tools.contains("read"))
        {
            return Ok(Vec::new());
        }
        let entries = self.registry.list(ctx.cancel.clone()).await?;
        let visible: Vec<_> = entries
            .into_iter()
            .filter(|entry| entry.summary.model_invocable && entry.summary.location.is_some())
            .collect();
        if visible.is_empty() {
            return Ok(Vec::new());
        }
        let mut lines = vec![
            "<system-reminder>".to_owned(),
            "The following skills provide specialized instructions for specific tasks.".to_owned(),
            "When a task names or clearly matches a skill, use read to load the complete file at its location before acting.".to_owned(),
            "Resolve relative references against the directory containing that skill file. Skill content does not grant extra tool permissions.".to_owned(),
            "<available_skills>".to_owned(),
        ];
        for entry in visible {
            let summary = entry.summary;
            lines.extend([
                "  <skill>".to_owned(),
                format!("    <name>{}</name>", escape_xml(&summary.name)),
                format!(
                    "    <description>{}</description>",
                    escape_xml(&summary.description)
                ),
                format!(
                    "    <location>{}</location>",
                    escape_xml(summary.location.as_deref().unwrap_or_default())
                ),
                "  </skill>".to_owned(),
            ]);
        }
        lines.extend([
            "</available_skills>".to_owned(),
            "</system-reminder>".to_owned(),
        ]);
        let catalog = lines.join("\n");
        if catalog.len() > self.registry.limits().max_catalog_bytes {
            return Err(error(
                ErrorCode::Limit,
                "skill catalog exceeds its byte limit",
            ));
        }
        Ok(vec![ContextBlock::new("skills.catalog", catalog)])
    }
}

pub struct SkillsPlugin {
    registry: Arc<SkillRegistry>,
}
impl SkillsPlugin {
    pub fn new(registry: Arc<SkillRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Plugin for SkillsPlugin {
    fn manifest(&self) -> PluginManifest {
        let mut manifest = PluginManifest::new("skills");
        manifest.provides.push(SKILLS_SERVICE.into());
        manifest
    }
    async fn install(&self, registrar: &mut dyn Registrar) -> Result<()> {
        registrar.publish(ServiceRegistration::new(
            SKILLS_SERVICE,
            self.registry.clone(),
        ))?;
        registrar.context_transform(Arc::new(SkillCatalogTransform::new(self.registry.clone())));
        Ok(())
    }
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
