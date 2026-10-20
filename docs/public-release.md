# Public source snapshots

Keep ordinary development and its history in the private repository. Prepare a
separate public source tree from an explicitly selected committed revision:

```sh
python3 deploy/export_public.py --ref HEAD --out /tmp/sessiondock-public-preview --draft
```

The destination must not exist. The exporter reads its allowlist and public
document overlays from that same revision: `public/manifest.json` controls the
included source, browser tools, service templates and guides. Uncommitted work,
private `.git`, historical references, internal operational documents, fleet
targets and runtime data are not copied. The core `process-links` library is
included; `resource-agent` source and its standalone installation guide are
included as an optional component. Labdesk and FileDock source are not included.

The exporter checks documentation links and suite documentation in the resulting
tree. It rejects symlinks/gitlinks and known personal project-path/private-key
shapes in selected content. This focused scan is not a complete secret detector:
review the output and license notices before publication. A first public release
still needs the maintainer's decision on licensing and imported-code rights.

Without a root `LICENSE`, `LICENSE.md` or `LICENSE.txt`, only `--draft` is allowed.
Draft snapshots carry `PUBLICATION-DRAFT.txt`. Once licensing is settled, export
again without `--draft`; do not merely delete that marker from an unreviewed tree.

Require every workspace member and shared source dependency to be present in the
exported tree; check Cargo metadata before building. Outstanding distribution work
is tracked in [TODO](../TODO.md#public-distribution).

Validate the exported tree independently using its README build command and
browser suites. For a first public repository, `--init-git` optionally initializes
an empty Git repository on `main`; it creates no commit or remote and copies no
history. Make the first public commit from the reviewed files, then point that
new repository at the chosen public remote. Do not add the private repository as
a remote or merge its history.

For later releases, export into another new directory and review the file changes
against the independent public checkout. Keep ordinary public release history;
there is no need to force-push it or reinitialize it each time. The export command
never creates a remote, publishes files, or deploys a service.
