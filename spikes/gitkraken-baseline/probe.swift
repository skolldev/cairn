// Black-box latency probe: post a real input event, record the screen, time the first
// and last changed frames in a region. Throwaway spike for issue 7.
import AppKit
import CoreMedia
import CoreVideo
import Foundation
import ScreenCaptureKit

struct FrameRec { let tMs: Double; let diffPrev: Double; let diffBase: Double }

func hostMs() -> Double { CMClockGetTime(CMClockGetHostTimeClock()).seconds * 1000 }

final class Recorder: NSObject, SCStreamOutput {
    var prev: [UInt8]? ; var base: [UInt8]?
    var frames: [FrameRec] = []
    let lock = NSLock()
    var pixelDelta = 24
    func stream(_ s: SCStream, didOutputSampleBuffer sb: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen, sb.isValid,
              let atts = CMSampleBufferGetSampleAttachmentsArray(sb, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
              let a = atts.first, let raw = a[.status] as? Int,
              SCFrameStatus(rawValue: raw) == .complete,
              let pb = sb.imageBuffer else { return }
        let t = sb.presentationTimeStamp.seconds * 1000
        CVPixelBufferLockBaseAddress(pb, .readOnly); defer { CVPixelBufferUnlockBaseAddress(pb, .readOnly) }
        let w = CVPixelBufferGetWidth(pb), h = CVPixelBufferGetHeight(pb), bpr = CVPixelBufferGetBytesPerRow(pb)
        guard let p = CVPixelBufferGetBaseAddress(pb)?.assumingMemoryBound(to: UInt8.self) else { return }
        var cur = [UInt8](repeating: 0, count: w * h)
        for y in 0..<h { let row = p + y * bpr
            for x in 0..<w { let o = x * 4
                cur[y * w + x] = UInt8((Int(row[o]) + 2 * Int(row[o + 1]) + Int(row[o + 2])) / 4) } }
        func diff(_ b: [UInt8]) -> Double {
            var n = 0; for i in 0..<cur.count where abs(Int(cur[i]) - Int(b[i])) > pixelDelta { n += 1 }
            return Double(n) / Double(cur.count) }
        lock.lock(); defer { lock.unlock() }
        if base == nil { base = cur }
        frames.append(FrameRec(tMs: t, diffPrev: prev.map(diff) ?? 0, diffBase: diff(base!)))
        prev = cur
    }
    func rebase() { lock.lock(); base = prev; frames.removeAll(); lock.unlock() }
    func snapshot() -> [FrameRec] { lock.lock(); defer { lock.unlock() }; return frames }
}

func startStream(rect: CGRect, rec: Recorder) async throws -> SCStream {
    let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
    guard let d = content.displays.first(where: { CGDisplayBounds($0.displayID).contains(rect.origin) }) else { fatalError("no display at rect") }
    let b = CGDisplayBounds(d.displayID)
    let cfg = SCStreamConfiguration()
    cfg.sourceRect = CGRect(x: rect.minX - b.minX, y: rect.minY - b.minY, width: rect.width, height: rect.height)
    cfg.width = Int(rect.width); cfg.height = Int(rect.height)
    cfg.minimumFrameInterval = CMTime(value: 1, timescale: 240)
    cfg.pixelFormat = kCVPixelFormatType_32BGRA
    cfg.showsCursor = false; cfg.queueDepth = 8
    let s = SCStream(filter: SCContentFilter(display: d, excludingWindows: []), configuration: cfg, delegate: nil)
    try s.addStreamOutput(rec, type: .screen, sampleHandlerQueue: DispatchQueue(label: "frames", qos: .userInteractive))
    try await s.startCapture()
    return s
}

func mouse(_ type: CGEventType, _ p: CGPoint) { CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: p, mouseButton: .left)?.post(tap: .cghidEventTap) }
func sleepMs(_ ms: Double) async { try? await Task.sleep(nanoseconds: UInt64(ms * 1e6)) }
func click(_ p: CGPoint) async -> Double {
    let t0 = hostMs(); mouse(.leftMouseDown, p); await sleepMs(10); mouse(.leftMouseUp, p); return t0 }
func scroll(_ px: Int32) { CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: px, wheel2: 0, wheel3: 0)?.post(tap: .cghidEventTap) }

func report(_ fs: [FrameRec], t0: Double, threshold: Double) -> [String: Any] {
    let after = fs.filter { $0.tMs >= t0 }
    let changed = after.filter { $0.diffPrev >= threshold }
    return ["first_ms": changed.first.map { $0.tMs - t0 } as Any? ?? NSNull(),
            "settle_ms": changed.last.map { $0.tMs - t0 } as Any? ?? NSNull(),
            "changed_frames": changed.count, "frames": after.count,
            "final_diff_base": after.last?.diffBase ?? 0]
}
func emit(_ o: Any) { let d = try! JSONSerialization.data(withJSONObject: o, options: [.sortedKeys]); print(String(data: d, encoding: .utf8)!); fflush(stdout) }
func rectArg(_ s: String) -> CGRect { let v = s.split(separator: ",").map { Double($0)! }; return CGRect(x: v[0], y: v[1], width: v[2], height: v[3]) }
func ptArg(_ s: String) -> CGPoint { let v = s.split(separator: ",").map { Double($0)! }; return CGPoint(x: v[0], y: v[1]) }

// --- calibrate: a window of ours flips black to white inside mouseDown.
final class Flip: NSView {
    var white = false
    override var acceptsFirstResponder: Bool { true }
    override func acceptsFirstMouse(for e: NSEvent?) -> Bool { true }
    override func mouseDown(with e: NSEvent) { white.toggle(); layer?.backgroundColor = (white ? NSColor.white : NSColor.black).cgColor; CATransaction.flush() }
}
func calibrate(n: Int) {
    let app = NSApplication.shared; app.setActivationPolicy(.accessory)
    let screen = NSScreen.screens.first { CGDisplayIsBuiltin($0.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as! CGDirectDisplayID) != 0 } ?? NSScreen.main!
    let f = NSRect(x: screen.frame.minX + 300, y: screen.frame.minY + 300, width: 400, height: 300)
    let w = NSWindow(contentRect: f, styleMask: [.borderless], backing: .buffered, defer: false)
    let v = Flip(frame: NSRect(origin: .zero, size: f.size)); v.wantsLayer = true; v.layer?.backgroundColor = NSColor.black.cgColor
    w.contentView = v; w.level = .floating; w.makeKeyAndOrderFront(nil); app.activate(ignoringOtherApps: true)
    let mainH = NSScreen.screens[0].frame.height
    let cg = CGRect(x: f.minX, y: mainH - f.maxY, width: f.width, height: f.height)
    let id = screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as! CGDirectDisplayID
    print("display", id, "hz", CGDisplayCopyDisplayMode(id)?.refreshRate ?? -1, "rect", cg)
    Task {
        let rec = Recorder(); let s = try await startStream(rect: cg.insetBy(dx: 50, dy: 50), rec: rec)
        await sleepMs(500); var out: [Double] = []
        let p = CGPoint(x: cg.midX, y: cg.midY); mouse(.mouseMoved, p); await sleepMs(200)
        for _ in 0..<n {
            await sleepMs(Double.random(in: 180...280)); rec.rebase()
            let t0 = await click(p); await sleepMs(180)
            if let f = report(rec.snapshot(), t0: t0, threshold: 0.5)["first_ms"] as? Double { out.append(f) }
        }
        try await s.stopCapture()
        emit(["kind": "calibrate", "n": out.count, "ms": out.map { ($0 * 100).rounded() / 100 }])
        exit(0)
    }
    app.run()
}

// --- control: repaint our own window on every display refresh, to find the recorder's ceiling.
final class Spin: NSView {
    var v: CGFloat = 0
    @objc func tick(_ l: CADisplayLink) { v = v > 0.9 ? 0 : v + 0.11; layer?.backgroundColor = NSColor(white: v, alpha: 1).cgColor }
}
func control(seconds: Double) {
    let app = NSApplication.shared; app.setActivationPolicy(.accessory)
    let screen = NSScreen.screens.first { CGDisplayIsBuiltin($0.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as! CGDirectDisplayID) != 0 } ?? NSScreen.main!
    let f = NSRect(x: screen.frame.minX + 420, y: screen.frame.minY + 57, width: 950, height: 900)
    let w = NSWindow(contentRect: f, styleMask: [.borderless], backing: .buffered, defer: false)
    let v = Spin(frame: NSRect(origin: .zero, size: f.size)); v.wantsLayer = true
    w.contentView = v; w.level = .floating; w.makeKeyAndOrderFront(nil); app.activate(ignoringOtherApps: true)
    let link = v.displayLink(target: v, selector: #selector(Spin.tick(_:))); link.add(to: .main, forMode: .common)
    let mainH = NSScreen.screens[0].frame.height
    let cg = CGRect(x: f.minX, y: mainH - f.maxY, width: f.width, height: f.height)
    Task {
        let rec = Recorder(); let s = try await startStream(rect: cg, rec: rec)
        await sleepMs(500); rec.rebase(); await sleepMs(seconds * 1000); try await s.stopCapture()
        let fs = rec.snapshot().filter { $0.diffPrev > 0 }; let gaps = zip(fs.dropFirst(), fs).map { $0.tMs - $1.tMs }
        emit(["kind": "control", "seconds": seconds, "frames": fs.count, "gaps_ms": gaps.map { ($0 * 10).rounded() / 10 }]); exit(0)
    }
    app.run()
}

// --- watch: baseline, act, then record until quiet or timeout.
func watch(rect: CGRect, action: String, threshold: Double, quietMs: Double, timeoutMs: Double, minMs: Double) async throws {
    let rec = Recorder(); let s = try await startStream(rect: rect, rec: rec)
    await sleepMs(400); rec.rebase()
    var t0 = hostMs()
    if action.hasPrefix("click:") { let p = ptArg(String(action.dropFirst(6))); mouse(.mouseMoved, p); await sleepMs(120); rec.rebase(); t0 = await click(p) }
    else if action.hasPrefix("exec:") { let p = Process(); p.executableURL = URL(fileURLWithPath: "/bin/zsh"); p.arguments = ["-c", String(action.dropFirst(5))]; t0 = hostMs(); try p.run() }
    else if action.hasPrefix("key:") { let k = CGKeyCode(action.dropFirst(4))!; t0 = hostMs()
        CGEvent(keyboardEventSource: nil, virtualKey: k, keyDown: true)?.post(tap: .cghidEventTap); CGEvent(keyboardEventSource: nil, virtualKey: k, keyDown: false)?.post(tap: .cghidEventTap) }
    while true {
        await sleepMs(50); let now = hostMs(); let ch = rec.snapshot().filter { $0.tMs >= t0 && $0.diffPrev >= threshold }
        if now - t0 >= timeoutMs { break }
        if let l = ch.last, now - l.tMs >= quietMs, now - t0 >= minMs { break }
    }
    try await s.stopCapture()
    var r = report(rec.snapshot(), t0: t0, threshold: threshold); r["kind"] = "watch"; r["action"] = action
    r["series"] = rec.snapshot().filter { $0.tMs >= t0 && $0.diffPrev >= threshold }.prefix(400).map { [($0.tMs - t0).rounded(), ($0.diffPrev * 1000).rounded() / 1000] }
    emit(r)
}

// --- scroll: drive the wheel for a fixed time and report presented-frame intervals.
func scrollRun(rect: CGRect, at: CGPoint, px: Int32, ticks: Int, everyMs: Double) async throws {
    let rec = Recorder(); let s = try await startStream(rect: rect, rec: rec)
    mouse(.mouseMoved, at); await sleepMs(400); rec.rebase(); let t0 = hostMs()
    for _ in 0..<ticks { scroll(px); await sleepMs(everyMs) }
    let t1 = hostMs(); await sleepMs(300); try await s.stopCapture()
    let f = rec.snapshot().filter { $0.tMs >= t0 && $0.tMs <= t1 && $0.diffPrev > 0 }
    let gaps = zip(f.dropFirst(), f).map { $0.tMs - $1.tMs }
    emit(["kind": "scroll", "seconds": (t1 - t0) / 1000, "frames": f.count, "gaps_ms": gaps.map { ($0 * 10).rounded() / 10 }])
}

let a = CommandLine.arguments
func opt(_ n: String, _ d: String) -> String { if let i = a.firstIndex(of: "--" + n), i + 1 < a.count { return a[i + 1] }; return d }
switch a.count > 1 ? a[1] : "" {
case "control": control(seconds: Double(opt("seconds", "5"))!)
case "calibrate": calibrate(n: Int(opt("n", "40"))!)
case "watch": Task { try await watch(rect: rectArg(opt("rect", "")), action: opt("action", ""), threshold: Double(opt("threshold", "0.002"))!, quietMs: Double(opt("quiet", "1000"))!, timeoutMs: Double(opt("timeout", "15000"))!, minMs: Double(opt("min", "0"))!); exit(0) }; dispatchMain()
case "scroll": Task { try await scrollRun(rect: rectArg(opt("rect", "")), at: ptArg(opt("at", "")), px: Int32(opt("px", "-24"))!, ticks: Int(opt("ticks", "300"))!, everyMs: Double(opt("every", "8"))!); exit(0) }; dispatchMain()
default: print("usage: probe calibrate|watch|scroll"); exit(2)
}
