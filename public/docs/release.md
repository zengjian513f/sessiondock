# Release packages

Run packaging from a Git checkout of the reviewed public snapshot with Python
3.11 or newer (the packager uses the standard-library `tomllib` module):

```sh
python3 deploy/release.py --ref HEAD
python3 deploy/release.py --ref HEAD --component resource-agent
```

The default package contains `sessiondock`, `sessiondock-hub`,
`sessiondock-transfer`, `ptyhost`, and `web/`. It does not compile or install the
collector. The separate `resource-agent` package requires the Linux BPF compiler
and headers described in the [collector guide](resource-agent.md), and includes
the service files and installation guide.

Packages use committed content from `git archive`; uncommitted files are not
included. Build paths are remapped and archive timestamps use the commit date.
`--check` performs the existing two-build reproducibility check. Packaging does
not publish a tag, upload an artifact, or deploy a service.

## Release acceptance

Build and run the browser suites covering the release, record their outcomes,
and inspect any changed UI. Keep real CLI acceptance opt-in. Confirm the project
license and imported-code rights before distributing an initial release;
[third-party notices](../NOTICE.md) remain applicable.

The public repository's first commit should contain only the reviewed snapshot.
Later releases can have ordinary public-to-public history. Never add the private
repository as a remote or merge its Git history into the public repository.
