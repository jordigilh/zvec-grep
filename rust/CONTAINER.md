# Rust `zg` container

`rust/Containerfile` builds a runtime-only image for the Rust implementation.
The image contains the `zg` executable, zvec's native runtime library, and the
Jieba dictionary assets. It does not contain the Rust toolchain or source tree.

The default command is the safest MCP transport for a process supervisor:

```text
zg --server --stdio
```

The image is intended to be published as:

```text
quay.io/jordigilh/zvec-grep
```

## Local build

From the repository root:

```sh
podman build --file rust/Containerfile --tag quay.io/jordigilh/zvec-grep:local rust
```

Run it as a stdio MCP server with a workspace and persistent server-state
mounts:

```sh
podman run --rm --interactive \
  --volume "$PWD:/workspace" \
  --volume "zvec-grep-home:/var/lib/zg" \
  quay.io/jordigilh/zvec-grep:local
```

MCP roots passed by the client must use the container path (`/workspace`), not
the host path. Workspace `.zvec-grep` indexes live in the workspace mount;
daemon state, logs, authorization, and model cache live in `/var/lib/zg`.

For a separate HTTP process, retain the loopback-only security default:

```sh
podman run --rm \
  --publish 127.0.0.1:7999:7999 \
  --volume "$PWD:/workspace" \
  --volume "zvec-grep-home:/var/lib/zg" \
  quay.io/jordigilh/zvec-grep:local \
  --server run --listen 127.0.0.1:7999
```

The MCP endpoint is `http://127.0.0.1:7999/mcp`. A gateway in another
container cannot reach that loopback address unless both services share a pod
or network namespace. Do not change the bind address to `0.0.0.0` without
adding an authenticated internal transport design.

## CI publication

`.github/workflows/publish-container.yml` builds `linux/amd64` and
`linux/arm64`. Pull requests build without publishing. The workflow publishes
on pushes to `main`, `zg-v*` tags, and manual dispatch when `publish` is true.

The workflow expects these repository Actions secrets:

- `QUAY_USERNAME`: a Quay robot-account username or user name;
- `QUAY_TOKEN`: the corresponding Quay token/password.

Set them with `gh secret set`; never commit them or put them in workflow YAML.

```zsh
REPO=jordigilh/zvec-grep
gh auth status

read -r "QUAY_USERNAME?Quay username: "
read -r -s "QUAY_TOKEN?Quay token: "
print

printf '%s' "$QUAY_USERNAME" | gh secret set QUAY_USERNAME --repo "$REPO"
printf '%s' "$QUAY_TOKEN" | gh secret set QUAY_TOKEN --repo "$REPO"
unset QUAY_USERNAME QUAY_TOKEN

gh secret list --repo "$REPO"
```

Create the Quay robot account first and grant it write permission to the
`zvec-grep` repository. Its robot username and token are the two values above.
