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

## App payload

effectcraft uses a complete .ecbundle provider envelope containing the unchanged native .ecproj document
(with its saved editor view) and every referenced media file; native schema validation and all footage/proxy/sequence files are retained. The envelope is versioned
ACFPB001, stored uncompressed, with bounded lengths and CRC32 for each entry. CRC detects accidental
corruption; this format is not an authentication mechanism. Media paths are rebased into a fresh,
private browser-table root on each open; they never become OS paths or provider resource IDs.
Downloaded bundles can be reopened through the browser's project picker or drag/drop. Desktop
native project support does not require understanding the optional transport envelope.
EffectCraft also packages supported glTF/OBJ material sidecars and File properties such as LUTs. External network URIs, parent-relative model sidecars, material-map options and externally linked OCIO/C4D/FBX formats fail closed until a lossless adapter exists.

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

Contribution credit: @alejocapo04.
