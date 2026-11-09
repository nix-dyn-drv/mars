//! Splits the crates of a crates.io index checkout into shards that cargo can
//! resolve: `<out>/<n>/Cargo.toml`, each with a Cargo.lock. Cargo's resolver
//! reads the checkout as a local registry, so yanked entries and entries its
//! parser rejects ("index entry is invalid") are skipped exactly as cargo skips
//! them, and every crate's version is whatever cargo picks when resolving the
//! crate on its own. A shard takes a crate when the resolution cargo found for
//! it agrees with the shard's: one version per semver-compatible range of each
//! package, one version per dependency of each package, one package per
//! `links` value, and no dependency cycle once the features everyone activates
//! are combined, which is what one resolution allows. Each shard is then
//! resolved as a whole with those versions locked in, the way a Cargo.lock
//! would; a crate cargo still objects to moves on to the next shard it fits.
//!
//! The resolutions are appended to `<out>/resolved.tsv` as they are found and
//! reused by later runs; delete the file after updating the index.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use cargo::GlobalContext;
use cargo::core::registry::PackageRegistry;
use cargo::core::resolver::{Resolve, ResolveError, ResolveOpts, ResolveVersion, VersionPreferences};
use cargo::core::{Dependency, PackageId, Registry, SourceId, Workspace};
use cargo::sources::source::QueryKind;
use cargo::sources::{IndexSummary, SourceConfigMap};
use cargo::util::interning::InternedString;
use cargo_util_terminal::Shell;
use futures::executor::block_on;

/// Crates per shard. Resolving a shard costs memory in proportion.
const SHARD_CAP: usize = 50_000;
/// Crates resolved per registry before it is replaced, which bounds its index cache.
const REGISTRY_LIFETIME: usize = 1_000;

const HEADER: &str = "[package]
name = \"MARS\"
version = \"0.0.0\"
autobins = false
autolibs = false
autotests = false
autobenches = false
autoexamples = false

[[bin]]
name = \"dummy\"
path = \"/dev/null\"

[workspace]

[dependencies]
";

/// What cargo found when resolving one crate on its own.
struct Member {
    id: PackageId,
    packages: Vec<PackageId>,
    /// The edges cargo's cycle check follows.
    edges: Vec<(PackageId, PackageId)>,
    links: Vec<(InternedString, PackageId)>,
}

impl Member {
    fn new(root: PackageId, resolve: &Resolve) -> Member {
        let packages: Vec<PackageId> = resolve.iter().filter(|&p| p != root).collect();
        let mut edges: Vec<(PackageId, PackageId)> =
            packages.iter().flat_map(|&p| resolve.transitive_deps_not_replaced(p).map(move |(d, _)| (p, d))).collect();
        // Sorted by source package and then dependency name, so `Shard::fits` walks a name at a time.
        edges.sort();
        Member {
            id: resolve.deps(root).next().map(|(id, _)| id).expect("the root depends on the crate"),
            edges,
            links: packages.iter().filter_map(|&p| resolve.summary(p).links().map(|l| (l, p))).collect(),
            packages,
        }
    }

    /// One line: name, version, the packages, their edges by package index, and a field per
    /// `links` value, which is free text, prefixed by its package index.
    fn save(&self, w: &mut impl Write) -> std::io::Result<()> {
        let at: HashMap<PackageId, usize> = self.packages.iter().enumerate().map(|(i, &p)| (p, i)).collect();
        let packages: Vec<String> = self.packages.iter().map(|p| format!("{}={}", p.name(), p.version())).collect();
        let edges: Vec<String> = self.edges.iter().map(|(a, b)| format!("{}>{}", at[a], at[b])).collect();
        let links: String = self.links.iter().map(|(l, p)| format!("\t{}:{l}", at[p])).collect();
        // One write, so an interrupted run leaves no partial line.
        let line = format!("{}\t{}\t{}\t{}{links}\n", self.id.name(), self.id.version(), packages.join(","), edges.join(","));
        w.write_all(line.as_bytes())
    }

    fn load(line: &str, source: SourceId) -> anyhow::Result<Member> {
        let fields: Vec<&str> = line.split('\t').collect();
        let [name, version, packages, edges, links @ ..] = &fields[..] else {
            anyhow::bail!("malformed line {line:?}");
        };
        let packages = packages
            .split(',')
            .map(|p| p.rsplit_once('=').ok_or_else(|| anyhow::anyhow!("malformed package {p:?}")).and_then(|(n, v)| PackageId::try_new(n, v, source)))
            .collect::<anyhow::Result<Vec<PackageId>>>()?;
        let at = |i: &str| -> anyhow::Result<PackageId> { packages.get(i.parse::<usize>()?).copied().ok_or_else(|| anyhow::anyhow!("no package {i}")) };
        let mut edges: Vec<(PackageId, PackageId)> = edges
            .split(',')
            .filter(|e| !e.is_empty())
            .map(|e| {
                let (a, b) = e.split_once('>').ok_or_else(|| anyhow::anyhow!("malformed edge {e:?}"))?;
                Ok((at(a)?, at(b)?))
            })
            .collect::<anyhow::Result<_>>()?;
        edges.sort();
        Ok(Member {
            id: packages.iter().copied().find(|p| &*p.name() == *name && p.version().to_string() == *version).ok_or_else(|| anyhow::anyhow!("{name} {version} is not among its packages"))?,
            edges,
            links: links
                .iter()
                .map(|l| {
                    let (i, l) = l.split_once(':').ok_or_else(|| anyhow::anyhow!("malformed links {l:?}"))?;
                    Ok((InternedString::new(l), at(i)?))
                })
                .collect::<anyhow::Result<_>>()?,
            packages,
        })
    }
}

#[derive(Default)]
struct Shard {
    versions: HashMap<(InternedString, (u64, u64, u64)), PackageId>,
    links: HashMap<InternedString, PackageId>,
    deps: HashMap<PackageId, Vec<PackageId>>,
    members: Vec<usize>,
}

impl Shard {
    fn fits(&self, m: &Member) -> bool {
        self.members.len() < SHARD_CAP
            && m.packages.iter().all(|p| self.versions.get(&range(*p)).is_none_or(|q| q == p))
            && m.links.iter().all(|(l, p)| self.links.get(l).is_none_or(|q| q == p))
            // A package's dependency on a name resolves to one version whatever ranges the shard
            // holds, though a package may declare a name twice under different names and may
            // activate only one of them: the versions seen for a name must nest.
            && m.edges.chunk_by(|a, b| a.0 == b.0 && a.1.name() == b.1.name()).all(|run| {
                let (from, name) = (run[0].0, run[0].1.name());
                let shard = || self.deps.get(&from).into_iter().flatten().filter(|q| q.name() == name);
                run.iter().all(|(_, to)| shard().any(|q| q == to)) || shard().all(|q| run.iter().any(|(_, to)| to == q))
            })
            && self.acyclic_with(&m.edges)
    }

    /// Adding edges creates a cycle exactly when the head of a new edge already reaches its tail.
    fn acyclic_with(&self, edges: &[(PackageId, PackageId)]) -> bool {
        let mut new: HashMap<PackageId, Vec<PackageId>> = HashMap::new();
        for &(from, to) in edges {
            if !self.deps.get(&from).is_some_and(|d| d.contains(&to)) {
                new.entry(from).or_default().push(to);
            }
        }
        let next = |p: &PackageId| self.deps.get(p).into_iter().chain(new.get(p)).flatten().copied();
        new.iter().all(|(&from, tos)| {
            tos.iter().all(|&to| {
                let (mut seen, mut stack) = (HashSet::new(), vec![to]);
                while let Some(p) = stack.pop() {
                    if p == from {
                        return false;
                    }
                    if seen.insert(p) {
                        stack.extend(next(&p));
                    }
                }
                true
            })
        })
    }

    fn add(&mut self, i: usize, m: &Member) {
        self.members.push(i);
        self.versions.extend(m.packages.iter().map(|&p| (range(p), p)));
        self.links.extend(m.links.iter().copied());
        for &(from, to) in &m.edges {
            let deps = self.deps.entry(from).or_default();
            if !deps.contains(&to) {
                deps.push(to);
            }
        }
    }
}

unsafe extern "C" {
    fn malloc_trim(pad: usize) -> i32;
}

/// The semver-compatible range a version belongs to; one resolution holds one version per range.
fn range(p: PackageId) -> (InternedString, (u64, u64, u64)) {
    let v = p.version();
    (p.name(), if v.major > 0 { (v.major, 0, 0) } else if v.minor > 0 { (0, v.minor, 0) } else { (0, 0, v.patch) })
}

fn main() -> anyhow::Result<()> {
    let (Some(index), Some(out)) = (std::env::args().nth(1), std::env::args().nth(2)) else {
        anyhow::bail!("usage: index-select <crates.io-index> <shards-out>");
    };
    let index = std::fs::canonicalize(index)?;
    std::fs::create_dir_all(&out)?;
    let out = std::fs::canonicalize(out)?;

    // A local registry is a directory whose `index` entry holds the index files.
    let registry = tempfile::tempdir()?;
    std::os::unix::fs::symlink(&index, registry.path().join("index"))?;
    let gctx = context(registry.path())?;

    let cache = out.join("resolved.tsv");
    let members: Vec<Member> = match std::fs::read_to_string(&cache) {
        Ok(text) => text.lines().map(|l| Member::load(l, SourceId::crates_io(&gctx)?)).collect::<anyhow::Result<_>>()?,
        Err(_) => Vec::new(),
    };
    let cached: HashSet<String> = members.iter().map(|m| m.id.name().to_lowercase()).collect();
    let mut names: Vec<String> = Vec::new();
    for entry in walkdir::WalkDir::new(&index).sort_by_file_name().into_iter().filter_entry(|e| e.file_name() != ".git") {
        let entry = entry?;
        // Index file names are the lowercased crate names.
        if entry.depth() >= 2 && entry.file_type().is_file() && !cached.contains(&*entry.file_name().to_string_lossy()) {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    eprintln!("index-select: {} crates resolved before, {} to resolve", members.len(), names.len());

    let members = Mutex::new(members);
    let writer = Mutex::new(std::fs::OpenOptions::new().create(true).append(true).open(&cache)?);
    let (next, done) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                let (names, members, writer, next, done, registry) = (&names, &members, &writer, &next, &done, registry.path());
                scope.spawn(move || -> anyhow::Result<()> {
                    let gctx = context(registry)?;
                    let dir = tempfile::tempdir()?;
                    let mut packages = package_registry(&gctx)?;
                    let mut count = 0;
                    while let Some(name) = names.get(next.fetch_add(1, Ordering::Relaxed)) {
                        if let Some(m) = confirm(name, dir.path(), &gctx, &packages)? {
                            m.save(&mut *writer.lock().unwrap())?;
                            members.lock().unwrap().push(m);
                        }
                        count += 1;
                        if count % REGISTRY_LIFETIME == 0 {
                            packages = package_registry(&gctx)?;
                        }
                        let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                        if n % 25_000 == 0 {
                            eprintln!("index-select: {n} of {} crates resolved", names.len());
                        }
                    }
                    Ok(())
                })
            })
            .collect();
        workers.into_iter().try_for_each(|w| w.join().unwrap())
    })?;
    // The workers' freed memory stays in their malloc arenas, which the shard resolves below
    // cannot reuse; hand it back to the OS instead of growing by a registry per shard.
    // SAFETY: glibc's malloc_trim only releases free heap pages.
    unsafe { malloc_trim(0) };
    let mut members = members.into_inner().unwrap();
    members.sort_by_key(|m| m.id.name());
    eprintln!("index-select: {} crates resolved by cargo", members.len());

    let mut shards: Vec<Shard> = Vec::new();
    for (i, m) in members.iter().enumerate() {
        let k = shards.iter().position(|s| s.fits(m)).unwrap_or_else(|| {
            shards.push(Shard::default());
            shards.len() - 1
        });
        shards[k].add(i, m);
    }
    eprintln!("index-select: {} shards", shards.len());

    let mut packages = package_registry(&gctx)?;
    let mut k = 0;
    while k < shards.len() {
        let dir = out.join((k + 1).to_string());
        std::fs::create_dir_all(&dir)?;
        let deps: String = shards[k].members.iter().map(|&i| format!("{} = \"={}\"\n", members[i].id.name(), members[i].id.version())).collect();
        // The registry's index cache stays warm across the many small shards; a large shard
        // would fill it with most of the index, so it gets a fresh one, as does every 400th.
        if shards[k].members.len() > 10_000 || k % 400 == 399 {
            packages = package_registry(&gctx)?;
        }
        // Lock every planned edge as a Cargo.lock would, so cargo follows the members' own
        // resolutions instead of trying newer versions it does not always find its way back from.
        packages.clear_lock();
        for (&p, planned) in &shards[k].deps {
            packages.register_lock(p, planned.clone());
        }
        match resolve(&dir, &deps, &gctx, &packages, true) {
            Ok(_) => {
                eprintln!("index-select: shard {} resolved with {} crates", k + 1, shards[k].members.len());
                k += 1;
            }
            Err(e) => {
                // The error's path runs from the package whose dependency failed up to the root.
                let path = e.downcast_ref::<ResolveError>().map_or(&[][..], |e| e.package_path());
                let Some(culprit) = path.len().checked_sub(2).map(|i| path[i]) else {
                    return Err(e.context(format!("shard {}", k + 1)));
                };
                let Some(pos) = shards[k].members.iter().position(|&i| members[i].id == culprit) else {
                    return Err(e.context(format!("shard {}: `{culprit}` is not a member", k + 1)));
                };
                let i = shards[k].members.remove(pos);
                let j = (k + 1..shards.len()).find(|&j| shards[j].fits(&members[i])).unwrap_or_else(|| {
                    shards.push(Shard::default());
                    shards.len() - 1
                });
                shards[j].add(i, &members[i]);
                eprintln!("index-select: shard {}: moved `{culprit}` to shard {}: {e}", k + 1, j + 1);
            }
        }
    }
    for entry in std::fs::read_dir(&out)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().parse::<usize>().is_ok_and(|n| n > shards.len()) {
            std::fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

/// A cargo context that reads the local registry in place of crates.io.
fn context(registry: &Path) -> anyhow::Result<GlobalContext> {
    let mut gctx = GlobalContext::new(Shell::from_write(Box::new(std::io::sink())), std::env::current_dir()?, registry.join("home"));
    let sources = ["source.crates-io.replace-with=\"mars\"".to_string(), format!("source.mars.local-registry={:?}", registry)];
    gctx.configure(0, true, None, false, false, false, &None, &[], &sources)?;
    Ok(gctx)
}

fn package_registry(gctx: &GlobalContext) -> anyhow::Result<PackageRegistry<'_>> {
    let mut packages = PackageRegistry::new_with_source_config(gctx, SourceConfigMap::new(gctx)?)?;
    packages.lock_patches();
    Ok(packages)
}

/// Resolves the crate behind an index file, or returns `None` when cargo cannot.
fn confirm<'g>(file: &str, dir: &Path, gctx: &'g GlobalContext, packages: &PackageRegistry<'g>) -> anyhow::Result<Option<Member>> {
    // `Normalized` matches the lowercased file name regardless of the crate's case.
    let dep = Dependency::parse(file, None, SourceId::crates_io(gctx)?)?;
    let versions: Vec<_> = block_on(packages.query_vec(&dep, QueryKind::Normalized))?
        .into_iter()
        .filter_map(|s| match s {
            IndexSummary::Candidate(s) if s.name().eq_ignore_ascii_case(file) => Some(s),
            _ => None,
        })
        .collect();
    let Some(newest) = versions.iter().max_by_key(|s| s.version()) else { return Ok(None) };
    // `*` lets cargo fall back to older versions but excludes pre-releases, which the newest version covers.
    let resolved = [format!("{} = \"*\"\n", newest.name()), format!("{} = \"{}\"\n", newest.name(), newest.version())]
        .iter()
        .find_map(|deps| resolve(dir, deps, gctx, packages, false).ok());
    Ok(resolved.map(|(root, resolve)| Member::new(root, &resolve)))
}

/// Resolves a manifest of `HEADER` plus `deps` in `dir`, writing its Cargo.lock when `lock` is set.
fn resolve<'g>(dir: &Path, deps: &str, gctx: &'g GlobalContext, packages: &PackageRegistry<'g>, lock: bool) -> anyhow::Result<(PackageId, Resolve)> {
    let manifest = dir.join("Cargo.toml");
    std::fs::write(&manifest, format!("{HEADER}{deps}"))?;
    let ws = Workspace::new(&manifest, gctx)?;
    let root = ws.current()?.summary().clone();
    let roots = [(root.clone(), ResolveOpts::everything())];
    let mut resolve = cargo::core::resolver::resolve(&roots, &[], packages, &VersionPreferences::default(), ResolveVersion::with_rust_version(None), Some(gctx))?;
    if lock {
        cargo::ops::write_pkg_lockfile(&ws, &mut resolve)?;
    }
    Ok((root.package_id(), resolve))
}
