import Foundation
import UIKit
import Darwin

struct PlatformIdentity {
    let platform: String
    let compiledSimulator: Bool
    let machine: String
    let os: String
    let osBuild: String
    let simulatorModel: String?

    init() throws {
        #if targetEnvironment(simulator)
        compiledSimulator = true
        platform = "simulator"
        #else
        compiledSimulator = false
        platform = "physical"
        #endif
        var system = utsname()
        try requireResult(uname(&system) == 0, "cannot identify running platform")
        machine = withUnsafeBytes(of: &system.machine) { bytes in
            String(decoding: bytes.prefix { $0 != 0 }, as: UTF8.self)
        }
        os = UIDevice.current.systemVersion
        var size = 0
        try requireResult(sysctlbyname("kern.osversion", nil, &size, nil, 0) == 0 && size > 0,
                          "cannot identify OS build")
        var bytes = [CChar](repeating: 0, count: size)
        let status = bytes.withUnsafeMutableBytes {
            sysctlbyname("kern.osversion", $0.baseAddress, &size, nil, 0)
        }
        try requireResult(status == 0, "cannot read OS build")
        osBuild = String(decoding: bytes.prefix { $0 != 0 }.map { UInt8(bitPattern: $0) }, as: UTF8.self)
        simulatorModel = ProcessInfo.processInfo.environment["SIMULATOR_MODEL_IDENTIFIER"]
        if compiledSimulator {
            try requireResult(!(simulatorModel ?? "").isEmpty,
                              "simulator compile target lacks SIMULATOR_MODEL_IDENTIFIER evidence")
        } else {
            try requireResult(simulatorModel == nil &&
                              (machine.hasPrefix("iPhone") || machine.hasPrefix("iPad") || machine.hasPrefix("iPod")),
                              "physical compile target conflicts with runtime platform evidence")
        }
    }

    func runtimeRecord(metadata: [String: Any]) throws -> [String: Any] {
        var record: [String: Any] = ["type": "runtime", "platform": platform,
            "compiled_simulator": compiledSimulator, "machine": machine, "os": os]
        #if arch(arm64)
        record["architecture"] = "arm64"
        #elseif arch(x86_64)
        record["architecture"] = "x86_64"
        #else
        throw BenchmarkFailure.invalid("unsupported benchmark architecture")
        #endif
        if compiledSimulator {
            record["simulator_model_identifier"] = simulatorModel
            // kern.osversion in a simulator describes the host kernel, not the guest OS.
            record["host_kernel_build"] = osBuild
            for key in ["simulator_device_type", "simulator_runtime", "simulator_runtime_version", "simulator_runtime_build"] {
                guard let value = metadata[key] as? String, !value.isEmpty else {
                    throw BenchmarkFailure.invalid("missing build pin: \(key)")
                }
                record[key] = value
            }
            let runtimeVersion = metadata["simulator_runtime_version"] as? String
            try requireResult(runtimeVersion == os || runtimeVersion == os + ".0",
                              "executing simulator OS differs from build pin")
        } else {
            record["os_build"] = osBuild
        }
        return record
    }
}
