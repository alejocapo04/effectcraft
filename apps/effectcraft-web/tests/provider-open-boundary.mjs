// Real browser/WASM regression, composed by smoke.mjs --provider-open-boundary.
// Requires the existing built app, Node >=22/CDP runner and a fresh ?empty&noworkers page.
// No transport-format mock: B is captured by the real provider save and the EXACT bytes
// reach public openResource twice (dialog rejection, then successful idle decoding/staging).
// Effect's private draft/live-preview fields are not exposed: its checkpoint is checked
// behaviorally by preserving CompSettings and cancelling back to identical A.
//
// The caller owns browser startup/output directories. This module never starts a browser.
export async function checkProviderOpenBoundary({ evaluate, timeoutMs = 60000 }) {
  return evaluate("(" + pageBoundary.toString() + ")(" + JSON.stringify({ appName: "effectcraft", timeoutMs }) + ")", timeoutMs * 6);
}

async function pageBoundary({ appName, timeoutMs }) {
  const app = window[appName];
  const provider = window[appName + "FileProviders"];
  const film = appName === "filmcraft";
  const bundleExtension = film ? "fcbundle" : "ecbundle";
  const nativeExtension = film ? "fcproj" : "ecproj";
  const dialogName = film ? "SequenceSettings" : "CompSettings";
  const events = [];
  const check = (condition, message) => { if (!condition) throw new Error(appName + " provider boundary: " + message); };
  const wait = async (predicate, label) => {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      const value = await predicate();
      if (value) return value;
      await new Promise(resolve => setTimeout(resolve, 25));
    }
    throw new Error("Timed out: " + label);
  };
  const event = (type, requestId, start = 0) => wait(
    () => events.slice(start).find(item => item.type === type && (!requestId || item.requestId === requestId)),
    type + " " + (requestId || "")
  );
  const digest = async bytes => {
    check(bytes instanceof Uint8Array, "readFile/save did not return native bytes");
    const hash = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
    return Array.from(hash, value => value.toString(16).padStart(2, "0")).join("");
  };
  const settleFrames = () => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  const decodedFrame = async () => {
    // This production control request calls Session::render_rgba8_alpha with the page's live
    // MediaPool. File-table equality alone cannot establish that a restored asset decodes.
    const frame = await app.request("render.frame", { time: 0, max_side: 32, base64: true, write: false, transparent: true });
    check(frame.width === 32 && frame.height === 16 && typeof frame.png === "string" && frame.png.length < 65536, "unexpected rendered fixture frame");
    const png = Uint8Array.from(atob(frame.png), value => value.charCodeAt(0));
    const bitmap = await createImageBitmap(new Blob([png], { type: "image/png" }));
    try {
      const canvas = document.createElement("canvas");
      canvas.width = 32; canvas.height = 16;
      const context = canvas.getContext("2d");
      check(context, "could not inspect decoded composition pixels");
      context.drawImage(bitmap, 0, 0);
      const center = Array.from(context.getImageData(16, 8, 1, 1).data);
      const corner = Array.from(context.getImageData(0, 0, 1, 1).data);
      check(center.every((value, index) => Math.abs(value - [35, 121, 185, 255][index]) <= 2), "live page decoder lost media pixels: " + center);
      check(corner[3] === 0, "fixture must retain transparent space outside its footage layer");
      return { center, corner, pngSha256: await digest(png) };
    } finally {
      bitmap.close();
    }
  };
  const listener = ({ detail }) => events.push({ ...detail, bytes: detail.bytes ? new Uint8Array(detail.bytes) : undefined });
  check(app && provider && !provider.snapshot().connected, "requires a fresh app without another registered host");
  if (!film) {
    const query = new URLSearchParams(location.search);
    check(query.has("noworkers") || query.get("frameworkers") === "0", "media restore regression requires ?noworkers or ?frameworkers=0");
    await wait(() => app.info().frameWorkers?.workers === 0, "frame workers must not mask the page MediaPool regression");
  }
  window.addEventListener(appName + "-file-provider", listener);
  const menu = id => app.request("ui.menu.invoke", { id });
  const current = () => provider.snapshot().documents?.[0];
  const clean = () => wait(() => !provider.snapshot().pending && current()?.dirty === false && current().provider, "durable fixture ACK");
  const capture = async (label, resourceId) => {
    const start = events.length;
    await menu("file.save");
    const dialog = await event("saveDialog", null, start);
    provider.resolveSaveDialog(dialog.requestId, "provider", label + "." + bundleExtension);
    const save = await event("save", dialog.requestId, start);
    check(save.bytes instanceof Uint8Array && save.bytes.length > 0, "fixture save has no bytes");
    provider.saveResult(save.requestId, "saved", resourceId);
    await clean();
    return new Uint8Array(save.bytes);
  };
  const boundResource = async expected => {
    const start = events.length;
    await menu("file.save");
    const dialog = await event("saveDialog", null, start);
    check(dialog.resourceId === expected, "resource binding changed: " + JSON.stringify(dialog));
    provider.resolveSaveDialog(dialog.requestId, "cancel", "");
    await wait(() => !provider.snapshot().pending, "cancel binding probe");
  };
  const probe = "/provider-boundary-A." + nativeExtension;
  const table = async () => {
    const files = app.files().slice().sort((left, right) => left.path.localeCompare(right.path));
    check(files.length <= 32 && files.reduce((sum, file) => sum + file.size, 0) <= 4 * 1024 * 1024, "fixture file table exceeded its test bound");
    return Promise.all(files.map(async file => {
      const bytes = await app.readFile(file.path);
      check(bytes.length === file.size, "file table size disagrees with bytes: " + file.path);
      return { path: file.path, size: file.size, kind: file.kind ?? null, sha256: await digest(bytes) };
    }));
  };
  const snapshot = async () => {
    // Save a Copy is a native serializer probe; it neither acknowledges the provider nor
    // changes its document identity/checkpoint. Reuse the same test-owned table path.
    await app.execute("file.saveCopy", { path: probe });
    const inspect = await app.inspect();
    return {
      document: JSON.parse(JSON.stringify(current())),
      resources: provider.snapshot().resources,
      projectSha256: await digest(await app.readFile(probe)),
      files: await table(),
      dialog: inspect.dialog,
      draft: film ? inspect.ui.sequence_settings : null
    };
  };
  const same = (left, right, label) => check(JSON.stringify(left) === JSON.stringify(right), label + " changed");
  const sameContent = (left, right) => {
    same(left.document, right.document, "document/revision/checkpoint");
    same(left.resources, right.resources, "provider Recent resources");
    same(left.projectSha256, right.projectSha256, "serialized native project");
    same(left.files, right.files, "complete real file table paths/sizes/bytes");
  };
  try {
    await app.execute("file.newProject", film ? { name: "Provider B" } : {});
    if (film) await app.request("ui.set", { mode: "edit" });
    const connection = provider.registerProvider("boundary-test", "Boundary regression", "synthetic-account");
    const create = label => app.execute(film ? "file.newSequence" : "comp.new", { name: label, width: label === "B" ? 32 : 16, height: 16, duration: 1 });
    await create("B");
    const canvas = document.createElement("canvas");
    canvas.width = 16; canvas.height = 16;
    const paint = canvas.getContext("2d");
    check(paint, "could not create synthetic image");
    paint.fillStyle = "#2379b9"; paint.fillRect(0, 0, 16, 16);
    const blob = await new Promise(resolve => canvas.toBlob(resolve, "image/png"));
    check(blob && blob.size > 0, "synthetic media is empty");
    const image = new File([blob], "provider-boundary.png", { type: "image/png" });
    const imageSha256 = await digest(new Uint8Array(await image.arrayBuffer()));
    let imagePath;
    let originalDecodedFrame;
    if (film) {
      const imported = await app.importFiles([image]);
      check(imported.items?.length === 1 && !imported.errors?.length, "synthetic media import failed");
    } else {
      imagePath = await app.addFile(image);
      // addFile posts open_or_import before this control request is drained.
      await app.inspect();
      const imported = await app.execute("file.import", { paths: [imagePath], addToComp: true });
      check(imported.items?.length === 1 && !imported.errors?.length, "could not add real fixture footage to B's composition");
      originalDecodedFrame = await decodedFrame();
    }
    const bundle = await capture("B", "b");
    const bundleSha256 = await digest(bundle);
    await app.execute("file.newProject", film ? { name: "Provider A" } : {});
    await create("A");
    await capture("A", "a");
    provider.setResources(connection, [
      { id: "a", name: "A." + bundleExtension },
      { id: "b", name: "B." + bundleExtension }
    ]);
    await settleFrames();

    // The read starts BEFORE A's real dialog. No UI click behind a modal is required.
    const rejectedRequest = provider.requestOpen("b");
    await menu(film ? "sequence.settings" : "app.compSettings");
    await settleFrames();
    const before = await snapshot();
    check(before.document.dirty === false && before.document.provider, "A must be clean and bound so only the live dialog rejects B");
    check(before.dialog === dialogName, "the real A dialog did not open");
    if (film) check(before.draft && before.draft.width === 16, "A's actual sequence draft is not observable");
    provider.openResource(rejectedRequest, "B." + bundleExtension, bundle);
    const failure = await event("openFailed", rejectedRequest);
    check(String(failure.error) === "Save changes and finish pending editor work before opening a provider project", "rejection was not the live-editor guard: " + failure.error);
    check(!events.some(item => item.type === "opened" && item.requestId === rejectedRequest), "rejected B emitted opened");
    const afterFailure = await snapshot();
    sameContent(before, afterFailure);
    same(before.dialog, afterFailure.dialog, "live dialog");
    if (film) same(before.draft, afterFailure.draft, "actual sequence draft");
    await boundResource("a");

    // Effect's private live-preview checkpoint is tested behaviorally, not exposed or mocked.
    await app.request("ui.key", { key: "Escape" });
    await wait(async () => !(await app.inspect()).dialog, "Cancel A dialog");
    await settleFrames();
    const afterCancel = await snapshot();
    sameContent(before, afterCancel);
    await boundResource("a");
    await settleFrames();

    const acceptedRequest = provider.requestOpen("b");
    provider.openResource(acceptedRequest, "B." + bundleExtension, bundle);
    const opened = await event("opened", acceptedRequest);
    check(!events.some(item => item.type === "openFailed" && item.requestId === acceptedRequest), "idle B was rejected");
    await clean();
    check(current().id !== before.document.id && opened.documentId === current().id, "idle B did not replace A's identity");
    await boundResource("b");
    const staged = (await table()).filter(file => file.path.startsWith("/__file_provider/" + acceptedRequest + "/"));
    check(staged.some(file => file.path.endsWith("/provider-boundary.png") && file.sha256 === imageSha256), "idle success did not restore B's actual distinguishable media");
    let restoredDecodedFrame;
    let reopenedRequest;
    let reopenedDecodedFrame;
    if (!film) {
      restoredDecodedFrame = await decodedFrame();
      same(originalDecodedFrame, restoredDecodedFrame, "provider-restored composition pixels/PNG");
      // A second complete open retires only the old private staging root. The original local
      // asset stays available, while the new private path must decode without a page reload.
      reopenedRequest = provider.requestOpen("b");
      provider.openResource(reopenedRequest, "B." + bundleExtension, bundle);
      await event("opened", reopenedRequest);
      await clean();
      reopenedDecodedFrame = await decodedFrame();
      same(originalDecodedFrame, reopenedDecodedFrame, "second provider-restored composition pixels/PNG");
      const files = await table();
      check(!files.some(file => file.path.startsWith("/__file_provider/" + acceptedRequest + "/")), "obsolete provider file-table root was retained");
      check(files.some(file => file.path.startsWith("/__file_provider/" + reopenedRequest + "/") && file.sha256 === imageSha256), "current provider media was removed with the old root");
      check(await digest(await app.readFile(imagePath)) === imageSha256, "provider cleanup removed unrelated local media");
    }
    check(await digest(bundle) === bundleSha256, "the rejected/accepted bundle bytes were changed");
    return {
      ok: true, boundary: "public openResource -> production WASM tick/open/decode/staging",
      rejectedRequest, acceptedRequest, bundleBytes: bundle.length, bundleSha256,
      guardError: failure.error, mediaTableBeforeEntries: before.files.length, stagedBMedia: staged,
      originalDecodedFrame, restoredDecodedFrame, reopenedRequest, reopenedDecodedFrame,
      draftAssertion: film ? "actual serialized Sequence Settings draft equality" : "dialog persists and real Cancel preserves A checkpoint; private draft/live struct equality unobserved"
    };
  } finally {
    window.removeEventListener(appName + "-file-provider", listener);
  }
}
