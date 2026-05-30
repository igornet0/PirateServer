//! Placeholder substitution for custom `pirate-nginx-snippet.conf` templates.

use std::path::Path;

/// Replace `<PATH_PROJECT>`, `<VERSION>`, and `<RELEASE_ROOT>` in a nginx template.
pub fn substitute_nginx_template(content: &str, project_root: &Path, version: &str) -> String {
    let project = project_root.to_string_lossy();
    let release_root = format!("{}/releases/{}", project, version);
    content
        .replace("<PATH_PROJECT>", &project)
        .replace("<VERSION>", version)
        .replace("<RELEASE_ROOT>", &release_root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn substitutes_all_placeholders() {
        let root = PathBuf::from("/var/lib/pirate/deploy/projects/p-test");
        let out = substitute_nginx_template(
            "root <PATH_PROJECT>/releases/<VERSION>/dist;\n# <RELEASE_ROOT>",
            &root,
            "0.1.1",
        );
        assert!(out.contains("root /var/lib/pirate/deploy/projects/p-test/releases/0.1.1/dist;"));
        assert!(out.contains("# /var/lib/pirate/deploy/projects/p-test/releases/0.1.1"));
        assert!(!out.contains("<PATH_PROJECT>"));
        assert!(!out.contains("<VERSION>"));
        assert!(!out.contains("<RELEASE_ROOT>"));
    }
}
