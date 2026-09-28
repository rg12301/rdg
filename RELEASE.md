# Release Process

This document describes how to release `rdg` to the community.

## Automated Release (Recommended)

The release process is fully automated via GitHub Actions. Simply push a version tag, and GitHub Actions will:

1. **Build binaries** for all platforms (macOS, Linux, Windows)
2. **Create a GitHub Release** with all binaries attached
3. **Publish to crates.io** (if you've set up `CARGO_REGISTRY_TOKEN`)

### Quick Start

```bash
# Tag a release (using semantic versioning)
git tag v1.1.0

# Push the tag (this triggers the release workflow)
git push origin v1.1.0
```

### What Happens Next

1. GitHub Actions starts building binaries for:
   - macOS (x86_64 Intel and aarch64 Apple Silicon)
   - Linux (x86_64 and aarch64)
   - Windows (x86_64)

2. Once all builds complete (~10-15 minutes), a new Release is created on GitHub with:
   - All binaries as downloadable assets
   - Auto-generated release notes from commit history

3. If `CARGO_REGISTRY_TOKEN` is configured in repo secrets, the workflow will also publish all crates to crates.io

## Prerequisites

### GitHub Secrets Setup

To enable automatic publishing to crates.io, configure this secret in your repository:

1. Go to **Settings → Secrets and variables → Actions**
2. Add `CARGO_REGISTRY_TOKEN` with your crates.io API token from [crates.io/me](https://crates.io/me)

If you don't want to publish to crates.io, the release workflow will still work — it will just skip the publish step.

## Platform-Specific Notes

### macOS

- **Intel (x86_64)**: Compatible with macOS 10.7+
- **Apple Silicon (aarch64)**: Compatible with macOS 11+

Both binaries are universal and can run on their respective architectures without installation.

### Linux

- **x86_64**: Requires glibc 2.31+ (Ubuntu 20.04+, Debian 11+, etc.)
- **aarch64**: Requires glibc 2.31+ on ARM64 systems

### Windows

- **x86_64**: Requires Windows 7 SP1 or later
- No dependencies; fully statically linked

## Verifying a Release

After a release is published:

1. Check the [Releases page](https://github.com/rg12301/rdg/releases)
2. Download the binary for your platform
3. Verify it works:
   ```bash
   chmod +x ./rdg  # Make it executable (macOS/Linux)
   ./rdg --version
   ```

## Manual Release (if needed)

If GitHub Actions fails or you need to manually create a release:

```bash
# Build locally for your platform
cargo build --release

# Create a release on GitHub
gh release create v1.1.0 \
  target/release/rdg \
  --title "v1.1.0" \
  --generate-notes

# Publish to crates.io
cargo publish --package rdg-cli
cargo publish --package rdg-schema
cargo publish --package rdg-graph
# ... (repeat for all crates)
```

## Workflow File

The release workflow is defined in [`.github/workflows/release.yml`](.github/workflows/release.yml).

It:
- Triggers on any tag matching `v*`
- Builds each platform in parallel
- Creates a single GitHub release with all binaries
- Publishes to crates.io (if token is configured)

## FAQ

**Q: Can I make pre-releases?**  
A: Yes! The workflow treats all tags as regular releases. For pre-releases, you can manually edit the GitHub release page after it's created.

**Q: What if a build fails?**  
A: The workflow stops and creates no release. Check the Actions tab to see which platform failed and why, fix it in code, and retry by pushing a new tag.

**Q: How do I skip crates.io publishing?**  
A: Either don't configure `CARGO_REGISTRY_TOKEN`, or the workflow will simply skip publish steps if the token is missing.

**Q: Can I release from a branch other than master?**  
A: Yes, the workflow triggers on any tag matching `v*`, regardless of which branch the tag points to.
