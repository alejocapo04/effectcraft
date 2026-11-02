# Optional external file providers

This web capability is **disabled by default**. Installing the WASM API does not register storage,
show provider recent files, or change local saving. The same-origin embedding host explicitly
registers one provider and one authenticated account namespace. Desktop and engine/CLI paths,
Save a Copy and exports retain their existing behavior. There is no bundled server, login,
filesystem mount or dependency on a private deployment.

The integration is a trusted same-origin capability, not a sandbox for arbitrary JavaScript.
The host authenticates users, filters every list/read/write by account, persists bytes durably,
and implements its destination dialog. The editor never interprets a resource ID as a path.
Do not put credentials in resource metadata.

## Protocol v1

API: window.effectcraftFileProviders; CustomEvent: effectcraft-file-provider. The frozen API exposes:

- registerProvider(providerId, label, namespace) -> connectionId. Identifiers use ASCII letters,
  digits, hyphen and underscore, at most 160 bytes; namespace identifies the authenticated account.
  Re-registering the exact identity is idempotent; an identity change requires unregistering first.
- unregisterProvider() and setEnabled(boolean), controlled by host configuration, not user preferences.
  Unregister refuses unresolved choices, captures and reads. Disabling does not discard pending work.
- setResources(connectionId, [{id,name,updatedAt?}]). A stale account connection is rejected.
  At most 100 unique entries, 255-byte simple filenames and 100-byte dates; no control characters.
- requestOpen(resourceId) -> requestId emits openRequest. The host returns
  openResource(requestId, filename, Uint8Array), or cancelOpen(requestId, error).
  Only a current requested resource can be delivered; opened is emitted after project decoding and
  complete media restoration. Failed or dirty-session opens emit openFailed and preserve the editor.
- resolveSaveDialog(requestId, destination, filename), where destination is provider, device or cancel.
- saveResult(requestId, status, resourceId), status saved, failed, cancel or download. **Only saved**
  means a durable provider checkpoint; download never acknowledges persistence or clears dirty state.
- snapshot() reports connected/enabled, provider identity, resources, active document and pending work.

Every interactive Save and Save As emits saveDialog with requestId, documentId, revision, name,
saveAs and the existing resourceId where relevant. Repeated Save while pending is coalesced.
After the choice, the editor freezes native document state and all supported media. A provider
save event owns bytes (Uint8Array), bytesLength, destination=provider and the exact captured
revision. The host MUST acknowledge only after durable persistence. Later edits remain dirty.
A device choice uses the original native serializer and browser download writer; it keeps provider
dirty state unchanged. Device formats and resource-linking behavior remain those of the official web app.
Cancel/failure never marks the project clean. Request IDs include a random page nonce and a
monotonic serial; delayed, duplicated and cross-account results cannot acknowledge another save.

All identity-bearing events include providerId, namespace and connectionId. Provider Recent is a
separate native UI section from this device's paths, including on Home where the app has one.
A currently modified project must be saved or explicitly closed before opening a provider file.
Pending operations block replacement commands and request the browser's usual unload warning.
The host must not claim a successful save for a native-project JSON with missing external media.

Disabling stops new provider choices while retaining document bindings and pending work. Interactive
Save and Save As on a linked document or pending save are explicitly refused while disabled. Re-enable
the provider to save it, or use Save a Copy for a device download. This refusal does not deliver bytes,
clear dirty state or acknowledge a provider checkpoint. Unlinked documents keep their original local
save behavior. Provider reads also recheck live dialogs and their preview checkpoints, editor drafts,
gestures, unconsumed input and running jobs immediately before decoding or staging media. A refused
read preserves the current project, resources and document identity. Finish the pending edit before retrying.

## App payload

effectcraft uses a complete .ecbundle provider envelope containing the unchanged native .ecproj document
(with its saved editor view) and every referenced media file; native schema validation and all footage/proxy/sequence files are retained. The envelope is versioned
ACFPB001, stored uncompressed, with bounded lengths and CRC32 for each entry. CRC detects accidental
corruption; this format is not an authentication mechanism. Media paths are rebased into a fresh,
private browser-table root on each open; they never become OS paths or provider resource IDs.
Downloaded bundles can be reopened through the browser's project picker or drag/drop. Desktop
native project support does not require understanding the optional transport envelope.
EffectCraft also packages supported glTF/GLB/OBJ material sidecars and File properties such as LUTs.
GLB v2 JSON chunks are inspected without decoding or copying their binary chunks. Container layout
follows [glTF 2.0.1, section 4.4.3](https://registry.khronos.org/glTF/specs/2.0/glTF-2.0.html#chunks):
a single leading JSON chunk, an optional second BIN chunk and four-byte chunk alignment; unknown
chunks are skipped. External buffer
and image URIs use the same relative-path policy as textual glTF, including native-loader percent
decoding before path checks. Models stay within the 80 MiB envelope limit, with at most 10,000 GLB
chunks and 10,000 distinct model dependencies. Embedded data URIs remain inside the model. Malformed/truncated GLB
containers, external network URIs, parent-relative model sidecars, material-map options and externally
linked OCIO/C4D/FBX formats fail closed until a lossless adapter exists. Missing transitive resources
are rejected before the current project, browser files or media decoder is changed.

Successful open registers every staged asset with the live page media decoder before emitting
`opened`, including when frame workers are disabled. A later provider open retires old private-root
files from both browser storage and the page decoder's encoded-byte/decoded caches. Cleanup uses
exact old staging paths and leaves the current root and unrelated local assets untouched.

Rebasing is limited to native footage/proxy/sequence references and File property values/keyframes. Text controls (including Str values), popup options and other project/view strings remain unchanged even when their contents equal a media path.

Each provider operation is bounded to **80 MiB**. Media/path metadata and decoded asset counts are
bounded independently. Large projects fail before a save event; local/native workflows remain
available. No cloud project is marked saved merely because serialization or a download completed.

## Validation

Source fixtures cover default-off behavior, account changes and stale connection lists, forged and
repeated acknowledgements, cancel/failure/download, bounded queues, resource aliases and correlated
opens. App fixtures cover bounded names, bundle integrity/media round trips and delayed-checkpoint
state. Run the normal repository unit/format/clippy/web-build checks before review. The embedding
host also needs browser integration tests for its authenticated persistence, dialog, media recovery
and failure handling. This feature does not claim that an arbitrary host authenticated or persisted
anything; those are responsibilities outside the editor.

The existing real-browser boundary regression also checks production `render.frame` pixels before
capture and after two provider opens on a fresh page with workers disabled. It verifies old staging
root cleanup and preservation of the original local image, in addition to its live-dialog rejection,
Cancel and resource-binding assertions. After building the current web app, run:

```sh
node apps/effectcraft-web/tests/smoke.mjs --provider-open-boundary --url 'http://127.0.0.1:8765/?empty&noworkers' --out target/web/provider-open-boundary
```

Native regressions exercise real PNG decoding and exact-path retirement in MediaPool, GLB external
buffer/image discovery, the production missing-sidecar preflight and malformed chunk rejection.
These source fixtures require execution under the normal gates. They do not establish browser or
host persistence behavior until the corresponding current-source run has completed.

Contribution credit: @alejocapo04.

## Native review previews

Use the existing snapshot example with `--empty --home-preview` for a provider-free Home
containing one synthetic device Recent. Add `--file-provider-preview` for the same Home with
synthetic external Recents. Plain `--empty` keeps the example's original empty-editor behavior.
These flags add no transport, storage service or authentication, and are off by default.
