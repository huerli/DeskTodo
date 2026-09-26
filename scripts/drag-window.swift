// 合成鼠标拖动，用于验证「拖动窗口时是否还在高频写盘」
// 用法: swift scripts/drag-window.swift <startX> <startY> <totalDx> <totalDy> <steps>
// 说明: 需要「辅助功能」权限才能投递 CGEvent；若被拒绝会打印提示。

import Cocoa

let args = CommandLine.arguments
guard args.count >= 6,
      let sx = Double(args[1]), let sy = Double(args[2]),
      let dx = Double(args[3]), let dy = Double(args[4]),
      let steps = Int(args[5]), steps > 0 else {
    FileHandle.standardError.write("用法: drag-window.swift <startX> <startY> <dx> <dy> <steps>\n".data(using: .utf8)!)
    exit(2)
}

let start = CGPoint(x: sx, y: sy)
let source = CGEventSource(stateID: .hidSystemState)

func post(_ type: CGEventType, _ p: CGPoint) {
    guard let e = CGEvent(mouseEventSource: source, mouseType: type,
                          mouseCursorPosition: p, mouseButton: .left) else { return }
    e.post(tap: .cghidEventTap)
}

guard CGPreflightPostEventAccess() else {
    FileHandle.standardError.write("缺少辅助功能权限，无法投递鼠标事件\n".data(using: .utf8)!)
    // 触发系统授权提示
    _ = CGRequestPostEventAccess()
    exit(3)
}

// 移动到位 → 按下 → 分步拖动 → 松开
post(.mouseMoved, start)
usleep(120_000)
post(.leftMouseDown, start)
usleep(60_000)

let stepDelay = 16_000 // ≈60fps
for i in 1...steps {
    let t = Double(i) / Double(steps)
    let p = CGPoint(x: start.x + dx * t, y: start.y + dy * t)
    post(.leftMouseDragged, p)
    usleep(useconds_t(stepDelay))
}

let end = CGPoint(x: start.x + dx, y: start.y + dy)
post(.leftMouseUp, end)
print("拖动完成: (\(Int(start.x)),\(Int(start.y))) → (\(Int(end.x)),\(Int(end.y)))，共 \(steps) 步")
