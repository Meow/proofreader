//! Configuration: loading `.proofreader.yml` files, `inherit_from`, per-directory discovery,
//! glob matching and per-reader settings.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::{env, fs, io};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use thiserror::Error;
use yaml_rust2::yaml::Hash;
use yaml_rust2::{Yaml, YamlLoader};

use crate::offense::Severity;
use crate::reader::{Reader, reader_named, registry};

/// File name of configuration files.
pub const CONFIG_FILE_NAME: &str = ".proofreader.yml";

/// Name of the section holding settings shared by all readers.
pub const ALL_READERS: &str = "AllReaders";

/// Key listing the files a configuration inherits from.
const INHERIT_FROM: &str = "inherit_from";

/// Keys every reader section understands; everything else is a reader option.
const STANDARD_KEYS: [&str; 6] = [
    "Description",
    "Enabled",
    "Severity",
    "AutoCorrect",
    "Include",
    "Exclude",
];

/// An error while loading a configuration file.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The file could not be read.
    #[error("cannot read {}: {source}", path.display())]
    Io {
        /// The file that failed.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// The file is not valid YAML or holds invalid settings.
    #[error("{}: {message}", path.display())]
    Invalid {
        /// The offending file.
        path: PathBuf,
        /// What is wrong.
        message: String,
    },
}

/// Builds a [`ConfigError::Invalid`].
fn invalid(path: &Path, message: impl Into<String>) -> ConfigError {
    ConfigError::Invalid {
        path: path.to_path_buf(),
        message: message.into(),
    }
}

/// Makes `path` absolute against the working directory and removes `.` and `..` components
/// lexically, without resolving symlinks.
pub fn absolute_path(path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
    };
    let mut normal = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normal.pop();
            }
            other => normal.push(other),
        }
    }
    normal
}

/// Shorthand for a YAML string key.
fn key(name: &str) -> Yaml {
    Yaml::String(name.to_owned())
}

/// A set of glob patterns resolved against the directory of the file that declared them.
///
/// Patterns starting with `**` or `/` are used as they are; others are relative to that
/// directory. A pattern without wildcards also matches everything below it.
#[derive(Debug, Clone, Default)]
pub struct Patterns {
    patterns: Vec<String>,
    set: GlobSet,
}

impl Patterns {
    /// Compiles `patterns` relative to `base`.
    pub fn new(patterns: Vec<String>, base: &Path) -> Result<Self, String> {
        let mut builder = GlobSetBuilder::new();
        for pattern in &patterns {
            for glob in expand_pattern(pattern, base) {
                let compiled = GlobBuilder::new(&glob)
                    .literal_separator(true)
                    .backslash_escape(true)
                    .build()
                    .map_err(|error| format!("invalid pattern `{pattern}`: {error}"))?;
                builder.add(compiled);
            }
        }
        let set = builder.build().map_err(|error| error.to_string())?;
        Ok(Patterns { patterns, set })
    }

    /// Whether no pattern was given.
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Whether the absolute `path` matches any pattern.
    pub fn matches(&self, path: &Path) -> bool {
        self.set.is_match(path)
    }

    /// The patterns as written in the configuration.
    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }
}

/// Turns a configured pattern into the absolute globs that implement it.
fn expand_pattern(pattern: &str, base: &Path) -> Vec<String> {
    let anchored = if pattern.starts_with("**") || Path::new(pattern).is_absolute() {
        pattern.to_owned()
    } else {
        let base = globset::escape(&base.to_string_lossy());
        format!(
            "{}/{}",
            base.trim_end_matches('/'),
            pattern.trim_start_matches("./")
        )
    };
    if pattern.contains(['*', '?', '[', '{']) {
        vec![anchored]
    } else {
        let below = format!("{}/**", anchored.trim_end_matches('/'));
        vec![anchored, below]
    }
}

/// The effective configuration of one reader for the files a [`Config`] applies to.
#[derive(Debug, Clone)]
pub struct ReaderConfig {
    /// `Enabled`.
    pub enabled: bool,
    /// `Severity`, falling back to `AllReaders.Severity`; `None` means the reader's default.
    pub severity: Option<Severity>,
    /// `AutoCorrect`.
    pub autocorrect: bool,
    include: Patterns,
    exclude: Patterns,
    /// The whole merged section (standard keys and options) as a YAML mapping.
    pub options: Yaml,
}

impl Default for ReaderConfig {
    fn default() -> Self {
        ReaderConfig {
            enabled: true,
            severity: None,
            autocorrect: true,
            include: Patterns::default(),
            exclude: Patterns::default(),
            options: Yaml::Hash(Hash::new()),
        }
    }
}

impl ReaderConfig {
    /// The raw value of `key`.
    pub fn get(&self, key: &str) -> Option<&Yaml> {
        self.options.as_hash()?.get(&Yaml::String(key.to_owned()))
    }

    /// `key` as a boolean, or `default` when missing or not a boolean.
    pub fn get_bool(&self, key: &str, default: bool) -> bool {
        self.get(key).and_then(Yaml::as_bool).unwrap_or(default)
    }

    /// `key` as a non-negative integer, or `default` when missing or not one.
    pub fn get_usize(&self, key: &str, default: usize) -> usize {
        match self.get(key) {
            Some(Yaml::Integer(value)) => usize::try_from(*value).unwrap_or(default),
            Some(Yaml::String(value)) => value.trim().parse().unwrap_or(default),
            _ => default,
        }
    }

    /// `key` as a string, or `default` when missing or not a string.
    pub fn get_str<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.get(key).and_then(Yaml::as_str).unwrap_or(default)
    }

    /// `key` as a list of strings (a single string counts as a one-element list), or `default`.
    pub fn get_str_list(&self, key: &str, default: &[&str]) -> Vec<String> {
        match self.get(key) {
            Some(Yaml::Array(items)) => items.iter().filter_map(scalar_string).collect(),
            Some(value @ (Yaml::String(_) | Yaml::Integer(_) | Yaml::Real(_))) => {
                scalar_string(value).into_iter().collect()
            }
            _ => default.iter().map(|item| (*item).to_owned()).collect(),
        }
    }

    /// The reader-level `Include` patterns.
    pub fn include(&self) -> &Patterns {
        &self.include
    }

    /// The reader-level `Exclude` patterns.
    pub fn exclude(&self) -> &Patterns {
        &self.exclude
    }

    /// Whether the reader-level `Include`/`Exclude` allow the absolute `path`.
    pub fn applies_to(&self, path: &Path) -> bool {
        (self.include.is_empty() || self.include.matches(path)) && !self.exclude.matches(path)
    }
}

/// Renders a scalar YAML value as a string.
fn scalar_string(value: &Yaml) -> Option<String> {
    match value {
        Yaml::String(text) | Yaml::Real(text) => Some(text.clone()),
        Yaml::Integer(number) => Some(number.to_string()),
        Yaml::Boolean(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// Raw configuration sections merged in inheritance order, before defaults are applied.
#[derive(Debug, Default)]
struct Layers {
    sections: BTreeMap<String, Hash>,
    bases: HashMap<(String, String), PathBuf>,
    origins: BTreeMap<String, PathBuf>,
}

impl Layers {
    /// Merges the file at `path` (after the files it inherits from) into the layers.
    fn load_file(&mut self, path: &Path, stack: &mut Vec<PathBuf>) -> Result<(), ConfigError> {
        let path = absolute_path(path);
        if stack.contains(&path) {
            return Err(invalid(&path, "circular inherit_from"));
        }
        let text = fs::read_to_string(&path).map_err(|source| ConfigError::Io {
            path: path.clone(),
            source,
        })?;
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        stack.push(path.clone());
        let result = self.load_text(&text, &path, &dir, stack);
        stack.pop();
        result
    }

    /// Merges YAML `text` that lives at `path` in `dir` into the layers.
    fn load_text(
        &mut self,
        text: &str,
        path: &Path,
        dir: &Path,
        stack: &mut Vec<PathBuf>,
    ) -> Result<(), ConfigError> {
        let documents =
            YamlLoader::load_from_str(text).map_err(|error| invalid(path, error.to_string()))?;
        let root = match documents.into_iter().next() {
            None | Some(Yaml::Null) => return Ok(()),
            Some(Yaml::Hash(root)) => root,
            Some(_) => return Err(invalid(path, "expected a mapping at the top level")),
        };
        if let Some(inherited) = root.get(&key(INHERIT_FROM)) {
            let files = match inherited {
                Yaml::String(file) => vec![file.clone()],
                Yaml::Array(files) => files
                    .iter()
                    .map(|file| {
                        file.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| invalid(path, "inherit_from entries must be strings"))
                    })
                    .collect::<Result<_, _>>()?,
                _ => return Err(invalid(path, "inherit_from must be a string or a list")),
            };
            for file in files {
                self.load_file(&dir.join(file), stack)?;
            }
        }
        for (name, value) in root {
            let Some(name) = name.as_str().map(str::to_owned) else {
                return Err(invalid(path, "top-level keys must be strings"));
            };
            if name == INHERIT_FROM {
                continue;
            }
            let entries = match value {
                Yaml::Hash(entries) => entries,
                Yaml::Null => Hash::new(),
                _ => return Err(invalid(path, format!("`{name}` must be a mapping"))),
            };
            self.origins
                .entry(name.clone())
                .or_insert_with(|| path.to_path_buf());
            let section = self.sections.entry(name.clone()).or_default();
            for (option, value) in entries {
                let Some(option_name) = option.as_str() else {
                    return Err(invalid(path, format!("keys in `{name}` must be strings")));
                };
                if option_name == "Include" || option_name == "Exclude" {
                    self.bases
                        .insert((name.clone(), option_name.to_owned()), dir.to_path_buf());
                }
                section.replace(option, value);
            }
        }
        Ok(())
    }

    /// Directory that relative `Include`/`Exclude` patterns of `section` resolve against.
    fn base(&self, section: &str, option: &str, fallback: &Path) -> PathBuf {
        self.bases
            .get(&(section.to_owned(), option.to_owned()))
            .cloned()
            .unwrap_or_else(|| fallback.to_path_buf())
    }
}

/// Collects validation errors while building a [`Config`].
struct Validator {
    errors: Vec<String>,
}

impl Validator {
    /// Reads an optional boolean setting.
    fn boolean(&mut self, value: Option<&Yaml>, default: bool, what: &str) -> bool {
        match value {
            None => default,
            Some(Yaml::Boolean(flag)) => *flag,
            Some(_) => {
                self.errors.push(format!("{what} must be true or false"));
                default
            }
        }
    }

    /// Reads an optional severity setting.
    fn severity(&mut self, value: Option<&Yaml>, what: &str) -> Option<Severity> {
        let value = value?;
        match value.as_str().map(str::parse::<Severity>) {
            Some(Ok(severity)) => Some(severity),
            Some(Err(error)) => {
                self.errors.push(format!("{what}: {error}"));
                None
            }
            None => {
                self.errors.push(format!("{what} must be a severity name"));
                None
            }
        }
    }

    /// Reads an optional list of glob patterns.
    fn patterns(
        &mut self,
        value: Option<&Yaml>,
        base: &Path,
        default: &[&str],
        what: &str,
    ) -> Patterns {
        let list = match value {
            None => default.iter().map(|item| (*item).to_owned()).collect(),
            Some(Yaml::Null) => Vec::new(),
            Some(Yaml::String(item)) => vec![item.clone()],
            Some(Yaml::Array(items)) => items.iter().filter_map(scalar_string).collect(),
            Some(_) => {
                self.errors
                    .push(format!("{what} must be a list of patterns"));
                Vec::new()
            }
        };
        Patterns::new(list, base).unwrap_or_else(|error| {
            self.errors.push(format!("{what}: {error}"));
            Patterns::default()
        })
    }
}

/// A fully merged configuration: defaults, inherited files and the file itself.
#[derive(Debug)]
pub struct Config {
    path: Option<PathBuf>,
    base_dir: PathBuf,
    include: Patterns,
    exclude: Patterns,
    severity: Option<Severity>,
    readers: BTreeMap<&'static str, ReaderConfig>,
    sections: BTreeMap<&'static str, Hash>,
    warnings: Vec<String>,
    fallback: ReaderConfig,
}

impl Config {
    /// The built-in defaults, with patterns relative to `base_dir`.
    pub fn defaults(base_dir: &Path) -> Config {
        Config::build(&Layers::default(), None, &absolute_path(base_dir)).0
    }

    /// Loads the configuration file at `path`, following `inherit_from`.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let path = absolute_path(path);
        let mut layers = Layers::default();
        layers.load_file(&path, &mut Vec::new())?;
        let base_dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        Config::finish(&layers, Some(path), &base_dir)
    }

    /// Builds a configuration from YAML text as if it were a file in `base_dir`.
    pub fn from_yaml_str(yaml: &str, base_dir: &Path) -> Result<Config, ConfigError> {
        let base_dir = absolute_path(base_dir);
        let path = base_dir.join(CONFIG_FILE_NAME);
        let mut layers = Layers::default();
        layers.load_text(yaml, &path, &base_dir, &mut vec![path.clone()])?;
        Config::finish(&layers, None, &base_dir)
    }

    /// Builds the configuration and turns validation errors into a [`ConfigError`].
    fn finish(
        layers: &Layers,
        path: Option<PathBuf>,
        base_dir: &Path,
    ) -> Result<Config, ConfigError> {
        let error_path = path
            .clone()
            .unwrap_or_else(|| base_dir.join(CONFIG_FILE_NAME));
        let (config, errors) = Config::build(layers, path, base_dir);
        if errors.is_empty() {
            Ok(config)
        } else {
            Err(invalid(&error_path, errors.join("; ")))
        }
    }

    /// Applies `layers` on top of the reader defaults, collecting validation errors.
    fn build(layers: &Layers, path: Option<PathBuf>, base_dir: &Path) -> (Config, Vec<String>) {
        let mut validator = Validator { errors: Vec::new() };
        let empty = Hash::new();
        let all = layers.sections.get(ALL_READERS).unwrap_or(&empty);
        let include = validator.patterns(
            all.get(&key("Include")),
            &layers.base(ALL_READERS, "Include", base_dir),
            &["**/*.lua"],
            "AllReaders.Include",
        );
        let exclude = validator.patterns(
            all.get(&key("Exclude")),
            &layers.base(ALL_READERS, "Exclude", base_dir),
            &[],
            "AllReaders.Exclude",
        );
        let severity = validator.severity(all.get(&key("Severity")), "AllReaders.Severity");
        let mut readers = BTreeMap::new();
        let mut sections = BTreeMap::new();
        for reader in registry() {
            let name = reader.name();
            let mut section = Hash::new();
            section.insert(key("Enabled"), Yaml::Boolean(true));
            section.insert(key("AutoCorrect"), Yaml::Boolean(true));
            for (option, value) in reader.default_options() {
                section.insert(key(option), value);
            }
            if let Some(user) = layers.sections.get(name) {
                for (option, value) in user {
                    section.replace(option.clone(), value.clone());
                }
            }
            let config = ReaderConfig {
                enabled: validator.boolean(
                    section.get(&key("Enabled")),
                    true,
                    &format!("{name}.Enabled"),
                ),
                severity: validator
                    .severity(section.get(&key("Severity")), &format!("{name}.Severity"))
                    .or(severity),
                autocorrect: validator.boolean(
                    section.get(&key("AutoCorrect")),
                    true,
                    &format!("{name}.AutoCorrect"),
                ),
                include: validator.patterns(
                    section.get(&key("Include")),
                    &layers.base(name, "Include", base_dir),
                    &[],
                    &format!("{name}.Include"),
                ),
                exclude: validator.patterns(
                    section.get(&key("Exclude")),
                    &layers.base(name, "Exclude", base_dir),
                    &[],
                    &format!("{name}.Exclude"),
                ),
                options: Yaml::Hash(section.clone()),
            };
            readers.insert(name, config);
            sections.insert(name, section);
        }
        let warnings = layers
            .origins
            .iter()
            .filter(|(name, _)| name.as_str() != ALL_READERS && reader_named(name).is_none())
            .map(|(name, origin)| {
                format!(
                    "Warning: unrecognized reader {name} found in {}",
                    origin.display()
                )
            })
            .collect();
        let config = Config {
            path,
            base_dir: base_dir.to_path_buf(),
            include,
            exclude,
            severity,
            readers,
            sections,
            warnings,
            fallback: ReaderConfig::default(),
        };
        (config, validator.errors)
    }

    /// The configuration file, or `None` for the built-in defaults.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The directory relative patterns are resolved against.
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// `AllReaders.Severity`, if set.
    pub fn default_severity(&self) -> Option<Severity> {
        self.severity
    }

    /// Whether `AllReaders.Include`/`Exclude` select the absolute `path`.
    pub fn includes_file(&self, path: &Path) -> bool {
        self.include.matches(path) && !self.exclude.matches(path)
    }

    /// Settings of the reader called `name` (defaults for unknown names).
    pub fn reader(&self, name: &str) -> &ReaderConfig {
        self.readers.get(name).unwrap_or(&self.fallback)
    }

    /// The severity `reader` reports with under this configuration.
    pub fn severity_of(&self, reader: &dyn Reader) -> Severity {
        self.reader(reader.name())
            .severity
            .unwrap_or_else(|| reader.default_severity())
    }

    /// Warnings about the configuration, such as unknown reader sections.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Renders the effective configuration of the readers `names` as YAML.
    pub fn describe(&self, names: &[&str]) -> String {
        let mut out = String::new();
        for (index, name) in names.iter().enumerate() {
            let (Some(reader), Some(section)) = (reader_named(name), self.sections.get(name))
            else {
                continue;
            };
            if index > 0 {
                out.push('\n');
            }
            let _ = writeln!(out, "{name}:");
            let _ = writeln!(out, "  Description: {}", yaml_scalar(reader.description()));
            let _ = writeln!(out, "  Enabled: {}", self.reader(name).enabled);
            let _ = writeln!(out, "  Severity: {}", self.severity_of(reader));
            let _ = writeln!(out, "  AutoCorrect: {}", self.reader(name).autocorrect);
            for option in ["Include", "Exclude"] {
                if let Some(value) = section.get(&key(option)) {
                    let _ = writeln!(out, "  {option}: {}", yaml_flow(value));
                }
            }
            for (option, value) in section {
                let Some(option) = option.as_str() else {
                    continue;
                };
                if !STANDARD_KEYS.contains(&option) {
                    let _ = writeln!(out, "  {option}: {}", yaml_flow(value));
                }
            }
        }
        out
    }
}

/// Renders a string as a plain YAML scalar, single-quoting it when needed.
fn yaml_scalar(text: &str) -> String {
    let special = text.is_empty()
        || text.starts_with([
            ' ', '-', '?', '&', '*', '!', '|', '>', '%', '@', '`', '\'', '"', '[', '{', '#',
        ])
        || text.ends_with(' ')
        || text.contains(": ")
        || text.contains(" #")
        || text.contains([',', '[', ']', '{', '}', '\n'])
        || matches!(text, "true" | "false" | "null" | "~" | "yes" | "no")
        || text.parse::<f64>().is_ok();
    if special {
        format!("'{}'", text.replace('\'', "''"))
    } else {
        text.to_owned()
    }
}

/// Renders a YAML value in flow style on one line.
fn yaml_flow(value: &Yaml) -> String {
    match value {
        Yaml::String(text) => yaml_scalar(text),
        Yaml::Real(text) => text.clone(),
        Yaml::Integer(number) => number.to_string(),
        Yaml::Boolean(flag) => flag.to_string(),
        Yaml::Array(items) => {
            let items: Vec<String> = items
                .iter()
                .map(|item| match item {
                    Yaml::String(text) => format!("'{}'", text.replace('\'', "''")),
                    other => yaml_flow(other),
                })
                .collect();
            format!("[{}]", items.join(", "))
        }
        Yaml::Hash(entries) => {
            let entries: Vec<String> = entries
                .iter()
                .map(|(name, value)| format!("{}: {}", yaml_flow(name), yaml_flow(value)))
                .collect();
            format!("{{ {} }}", entries.join(", "))
        }
        Yaml::Null | Yaml::BadValue | Yaml::Alias(_) => "~".to_owned(),
    }
}

/// Finds and caches the configuration that applies to each directory.
#[derive(Debug)]
pub struct ConfigStore {
    forced: Option<Arc<Config>>,
    defaults: Option<Arc<Config>>,
    discovered: HashMap<PathBuf, Option<PathBuf>>,
    loaded: HashMap<PathBuf, Arc<Config>>,
    warnings: Vec<String>,
}

impl ConfigStore {
    /// Creates a store; with `forced`, that file applies to every target.
    pub fn new(forced: Option<&Path>) -> Result<Self, ConfigError> {
        let mut store = ConfigStore {
            forced: None,
            defaults: None,
            discovered: HashMap::new(),
            loaded: HashMap::new(),
            warnings: Vec::new(),
        };
        if let Some(path) = forced {
            let config = Config::load(path)?;
            store.warnings.extend_from_slice(config.warnings());
            store.forced = Some(Arc::new(config));
        }
        Ok(store)
    }

    /// The configuration for the file at absolute `path`.
    pub fn for_file(&mut self, path: &Path) -> Result<Arc<Config>, ConfigError> {
        match path.parent() {
            Some(dir) => self.for_dir(dir),
            None => self.for_dir(path),
        }
    }

    /// The configuration for files in the absolute directory `dir`: the nearest
    /// `.proofreader.yml` in it or its ancestors, or the defaults.
    pub fn for_dir(&mut self, dir: &Path) -> Result<Arc<Config>, ConfigError> {
        if let Some(forced) = &self.forced {
            return Ok(Arc::clone(forced));
        }
        let Some(path) = self.discover(dir) else {
            let defaults = self
                .defaults
                .get_or_insert_with(|| Arc::new(Config::defaults(Path::new("."))));
            return Ok(Arc::clone(defaults));
        };
        if let Some(config) = self.loaded.get(&path) {
            return Ok(Arc::clone(config));
        }
        let config = Arc::new(Config::load(&path)?);
        self.warnings.extend_from_slice(config.warnings());
        self.loaded.insert(path, Arc::clone(&config));
        Ok(config)
    }

    /// Returns and clears the warnings of configurations loaded so far.
    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    /// Walks up from `dir` to the nearest configuration file, caching every visited directory.
    fn discover(&mut self, dir: &Path) -> Option<PathBuf> {
        let mut visited = Vec::new();
        let mut found = None;
        for ancestor in dir.ancestors() {
            if let Some(cached) = self.discovered.get(ancestor) {
                found = cached.clone();
                break;
            }
            visited.push(ancestor.to_path_buf());
            let candidate = ancestor.join(CONFIG_FILE_NAME);
            if candidate.is_file() {
                found = Some(candidate);
                break;
            }
        }
        for directory in visited {
            self.discovered.insert(directory, found.clone());
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("proofreader-config-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent dir");
        }
        fs::write(path, text).expect("write file");
    }

    #[test]
    fn absolute_paths_are_normalised() {
        assert_eq!(
            absolute_path(Path::new("/a/./b/../c")),
            PathBuf::from("/a/c")
        );
        assert!(absolute_path(Path::new("x.lua")).is_absolute());
    }

    #[test]
    fn defaults_include_lua_files_anywhere() {
        let config = Config::defaults(Path::new("/project"));
        assert!(config.includes_file(Path::new("/project/lib/a.lua")));
        assert!(config.includes_file(Path::new("/elsewhere/b.lua")));
        assert!(!config.includes_file(Path::new("/project/readme.md")));
        let length = config.reader("Layout/LineLength");
        assert!(length.enabled);
        assert!(length.autocorrect);
        assert_eq!(length.get_usize("Max", 0), 120);
        assert!(length.get_bool("AllowURI", false));
    }

    #[test]
    fn patterns_are_relative_to_the_config_directory() {
        let config = Config::from_yaml_str(
            "AllReaders:\n  Exclude:\n    - 'packages/pon/**/*'\n    - packages/yaml/lib/yaml.lua\n    - docs\n",
            Path::new("/flux"),
        )
        .expect("valid config");
        assert!(config.includes_file(Path::new("/flux/lib/a.lua")));
        assert!(!config.includes_file(Path::new("/flux/packages/pon/lib/pon.lua")));
        assert!(!config.includes_file(Path::new("/flux/packages/yaml/lib/yaml.lua")));
        assert!(config.includes_file(Path::new("/flux/packages/yaml/lib/install.lua")));
        assert!(!config.includes_file(Path::new("/flux/docs/deep/x.lua")));
        assert!(config.includes_file(Path::new("/other/packages/pon/lib/pon.lua")));
    }

    #[test]
    fn single_star_does_not_cross_directories() {
        let config =
            Config::from_yaml_str("AllReaders:\n  Exclude: ['lib/*.lua']\n", Path::new("/p"))
                .expect("valid");
        assert!(!config.includes_file(Path::new("/p/lib/a.lua")));
        assert!(config.includes_file(Path::new("/p/lib/sub/a.lua")));
    }

    #[test]
    fn reader_sections_merge_with_defaults() {
        let config = Config::from_yaml_str(
            "Layout/LineLength:\n  Max: 80\n  Severity: warning\n  Exclude: ['legacy.lua']\n",
            Path::new("/p"),
        )
        .expect("valid");
        let length = config.reader("Layout/LineLength");
        assert_eq!(length.get_usize("Max", 0), 80);
        assert!(length.get_bool("AllowURI", false));
        assert_eq!(length.severity, Some(Severity::Warning));
        assert!(!length.applies_to(Path::new("/p/legacy.lua")));
        assert!(length.applies_to(Path::new("/p/modern.lua")));
        assert!(
            config
                .reader("Layout/TrailingWhitespace")
                .applies_to(Path::new("/p/legacy.lua"))
        );
    }

    #[test]
    fn all_readers_severity_is_the_fallback() {
        let config = Config::from_yaml_str(
            "AllReaders:\n  Severity: refactor\nLayout/LineLength:\n  Severity: error\n",
            Path::new("/p"),
        )
        .expect("valid");
        assert_eq!(
            config.reader("Layout/LineLength").severity,
            Some(Severity::Error)
        );
        assert_eq!(
            config.reader("Layout/TrailingWhitespace").severity,
            Some(Severity::Refactor)
        );
        assert_eq!(config.default_severity(), Some(Severity::Refactor));
    }

    #[test]
    fn invalid_values_are_errors() {
        assert!(
            Config::from_yaml_str("Layout/LineLength:\n  Severity: loud\n", Path::new("/p"))
                .is_err()
        );
        assert!(
            Config::from_yaml_str("Layout/LineLength:\n  Enabled: maybe\n", Path::new("/p"))
                .is_err()
        );
        assert!(Config::from_yaml_str("- a\n- b\n", Path::new("/p")).is_err());
        assert!(Config::from_yaml_str("a: [\n", Path::new("/p")).is_err());
        assert!(Config::from_yaml_str("AllReaders:\n  Include: ['[']\n", Path::new("/p")).is_err());
        assert!(Config::from_yaml_str("", Path::new("/p")).is_ok());
    }

    #[test]
    fn unknown_readers_produce_warnings() {
        let config =
            Config::from_yaml_str("Foo/Bar:\n  Enabled: false\n", Path::new("/p")).expect("valid");
        assert_eq!(config.warnings().len(), 1);
        assert!(
            config.warnings()[0].starts_with("Warning: unrecognized reader Foo/Bar found in /p/")
        );
    }

    #[test]
    fn inheritance_merges_in_order() {
        let dir = scratch("inherit");
        write(
            &dir.join("base.yml"),
            "Layout/LineLength:\n  Max: 100\n  IgnoreComments: true\n  Exclude: ['generated/**/*']\n",
        );
        write(&dir.join("second.yml"), "Layout/LineLength:\n  Max: 90\n");
        write(
            &dir.join("sub/.proofreader.yml"),
            "inherit_from:\n  - ../base.yml\n  - ../second.yml\nLayout/LineLength:\n  AllowURI: false\n",
        );
        let config = Config::load(&dir.join("sub/.proofreader.yml")).expect("valid");
        let length = config.reader("Layout/LineLength");
        assert_eq!(length.get_usize("Max", 0), 90);
        assert!(length.get_bool("IgnoreComments", false));
        assert!(!length.get_bool("AllowURI", true));
        assert!(!length.applies_to(&dir.join("generated/x.lua")));
        assert!(length.applies_to(&dir.join("sub/generated/x.lua")));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn circular_inheritance_is_an_error() {
        let dir = scratch("circular");
        write(&dir.join("a.yml"), "inherit_from: b.yml\n");
        write(&dir.join("b.yml"), "inherit_from: a.yml\n");
        assert!(Config::load(&dir.join("a.yml")).is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn store_discovers_the_nearest_config() {
        let dir = scratch("discover");
        write(
            &dir.join(CONFIG_FILE_NAME),
            "Layout/LineLength:\n  Max: 100\n",
        );
        write(
            &dir.join("a/b/.proofreader.yml"),
            "Layout/LineLength:\n  Max: 50\n",
        );
        fs::create_dir_all(dir.join("a/c")).expect("dir");
        let mut store = ConfigStore::new(None).expect("store");
        let max = |config: Arc<Config>| config.reader("Layout/LineLength").get_usize("Max", 0);
        assert_eq!(max(store.for_dir(&dir.join("a/c")).expect("config")), 100);
        assert_eq!(max(store.for_dir(&dir.join("a/b")).expect("config")), 50);
        assert_eq!(
            max(store.for_file(&dir.join("a/b/x.lua")).expect("config")),
            50
        );
        assert_eq!(max(store.for_dir(&dir.join("a")).expect("config")), 100);
        let mut forced = ConfigStore::new(Some(&dir.join("a/b/.proofreader.yml"))).expect("store");
        assert_eq!(max(forced.for_dir(&dir).expect("config")), 50);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn describe_renders_yaml() {
        let config = Config::from_yaml_str(
            "Layout/LineLength:\n  Exclude: ['a.lua']\n",
            Path::new("/p"),
        )
        .expect("valid");
        let yaml = config.describe(&["Layout/LineLength"]);
        assert!(yaml.starts_with("Layout/LineLength:\n  Description: "));
        assert!(yaml.contains("  Enabled: true\n  Severity: convention\n  AutoCorrect: true\n"));
        assert!(yaml.contains("  Exclude: ['a.lua']\n"));
        assert!(yaml.contains("  Max: 120\n"));
        let parsed = YamlLoader::load_from_str(&yaml).expect("valid YAML");
        assert_eq!(parsed[0]["Layout/LineLength"]["Max"].as_i64(), Some(120));
    }

    #[test]
    fn typed_getters() {
        let config = Config::from_yaml_str(
            "Layout/LineLength:\n  Names: [a, 'b']\n  One: c\n  Count: '7'\n",
            Path::new("/p"),
        )
        .expect("valid");
        let length = config.reader("Layout/LineLength");
        assert_eq!(length.get_str_list("Names", &[]), vec!["a", "b"]);
        assert_eq!(length.get_str_list("One", &[]), vec!["c"]);
        assert_eq!(length.get_str_list("Missing", &["d"]), vec!["d"]);
        assert_eq!(length.get_usize("Count", 0), 7);
        assert_eq!(length.get_str("One", "x"), "c");
        assert_eq!(length.get_str("Missing", "x"), "x");
    }
}
