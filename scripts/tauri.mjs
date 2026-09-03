import { homedir } from "node:os";
import { join } from "node:path";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { execFileSync, spawn } from "node:child_process";

// Tauri invokes Cargo itself. Add Rustup's standard bin directory here so a
// terminal opened before PATH was refreshed can still start the application.
if (process.platform === "win32") {
  const cargoBin = join(homedir(), ".cargo", "bin");
  const parts = (process.env.PATH ?? "").split(";");
  if (!parts.some((part) => part.toLowerCase() === cargoBin.toLowerCase())) {
    process.env.PATH = `${cargoBin};${process.env.PATH ?? ""}`;
  }

  // Rust's MSVC target must run with MSVC's linker and SDK before Git Bash's
  // similarly named GNU `link.exe`. VsDevCmd emits the fully prepared env.
  const vswhere = join(process.env["ProgramFiles(x86)"] ?? "C:\\Program Files (x86)", "Microsoft Visual Studio", "Installer", "vswhere.exe");
  if (!existsSync(vswhere)) {
    console.error("Microsoft C++ Build Tools were not found. Install the VCTools workload, then rerun this command.");
    process.exit(1);
  }
  const installationPath = execFileSync(vswhere, ["-latest", "-products", "*", "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-property", "installationPath"], { encoding: "utf8" }).trim();
  const devCmd = join(installationPath, "Common7", "Tools", "VsDevCmd.bat");
  if (!installationPath || !existsSync(devCmd)) {
    console.error("Microsoft C++ Build Tools are installed but the VCTools workload is missing.");
    process.exit(1);
  }
  // Calling a quoted .bat via `cmd /c` is fragile when Node, Git Bash and cmd
  // all process the same quotation marks. A no-space temporary wrapper avoids
  // that second parsing step and leaves no file behind.
  const tempDirectory = mkdtempSync(join(process.env.TEMP ?? homedir(), "baobao-bashi-"));
  const environmentScript = join(tempDirectory, "load-msvc.cmd");
  writeFileSync(environmentScript, `@echo off\r\ncall "${devCmd}" -no_logo\r\nset\r\n`, "utf8");
  let output;
  try {
    output = execFileSync("cmd.exe", ["/d", "/c", environmentScript], { encoding: "utf8", windowsHide: true });
  } finally {
    rmSync(tempDirectory, { recursive: true, force: true });
  }
  for (const line of output.split(/\r?\n/)) {
    const delimiter = line.indexOf("=");
    if (delimiter > 0) process.env[line.slice(0, delimiter)] = line.slice(delimiter + 1);
  }
}

const command = process.argv[2];
const isCargoCheck = command === "cargo-check";
const isCargoRun = command === "cargo-run";
const isCargoTest = command === "cargo-test";
const isCargoCommand = isCargoCheck || isCargoRun || isCargoTest;
const executable = isCargoCommand ? "cargo" : (process.platform === "win32" ? "tauri.cmd" : "tauri");
const argumentsToPass = isCargoCheck ? ["check"] : isCargoRun ? ["run", "--no-default-features", "--color", "always", "--"] : isCargoTest ? ["test"] : process.argv.slice(2);
const child = spawn(executable, argumentsToPass, {
  stdio: "inherit",
  env: process.env,
  shell: process.platform === "win32",
  cwd: isCargoCommand ? join(process.cwd(), "src-tauri") : process.cwd()
});
child.on("error", (error) => {
  console.error(`Unable to start Tauri: ${error.message}`);
  process.exitCode = 1;
});
child.on("exit", (code) => { process.exitCode = code ?? 1; });
