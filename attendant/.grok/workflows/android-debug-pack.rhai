let meta = #{
    name: "android-debug-pack",
    description: "Check or pack the OpenBot Android debug APK. Default is check only.",
    when_to_use: "Empty .so, ccache zero-fill, Metro 127.0.0.1:8081, adb reverse, attendant debug phone",
    phases: [
        #{ title: "Check", detail: "ELF, Gradle, Metro, reverse" },
        #{ title: "Act", detail: "link, pack, repack, or connect only when mode asks" },
        #{ title: "Verify", detail: "a non-ELF library is a failure" },
    ],
};

let mode = "check";
let repo = "/tmp/slave-look";
if args != () {
    if args.mode != () { mode = args.mode; }
    if args.repo != () { repo = args.repo; }
}
let allowed = mode == "check" || mode == "link" || mode == "pack" || mode == "repack" || mode == "connect";
if !allowed {
    pause("verification", "mode must be check, link, pack, repack, or connect");
}

phase("Check");
let check_prompt = "Run this command and return its full stdout and exit code. Do not start Gradle, Metro, or a second build. Do not clean. Command: sh " + repo + "/scripts/android-debug-pack.sh check";
let checked = agent(check_prompt, #{
    label: "apk-check",
    capability_mode: "execute",
});

phase("Act");
let acted = ();
if mode != "check" {
    let act_prompt = "Run exactly: sh " + repo + "/scripts/android-debug-pack.sh " + mode + ". ";
    act_prompt += "Obey the script. Do not pass --build-cache. Do not run ninja -j with a number above 1. ";
    act_prompt += "Do not clean the android tree. Do not start a second Gradle or a second Metro. ";
    act_prompt += "If the script prints FAIL or exits non-zero, stop and quote the FAIL line. Return the full output.";
    acted = agent(act_prompt, #{
        label: "apk-" + mode,
        capability_mode: "execute",
    });
}

phase("Verify");
let verify_prompt = "Read the command output below. mode is " + mode + ". ";
verify_prompt += "For check: ok=true when MODE check is present and FAIL is absent. Quote any BAD or APK_SO line in evidence; those lines are findings, not a failed check. ";
verify_prompt += "For link, pack, repack, or connect: ok=true only when the output contains PACK_OK, REPACK_OK, or CONNECT_OK and does not contain FAIL. ";
verify_prompt += "Otherwise ok=false. Check output JSON: ";
if checked != () && checked.success {
    verify_prompt += json_encode(checked.output);
}
if acted != () && acted.success {
    verify_prompt += " Action output JSON: " + json_encode(acted.output);
}
let verdict = agent(verify_prompt, #{
    label: "apk-verify",
    capability_mode: "read-only",
    output_schema: #{
        "type": "object",
        "required": ["ok", "evidence"],
        "properties": #{
            "ok": #{ "type": "boolean" },
            "evidence": #{ "type": "string" },
        },
    },
});

if verdict != () && verdict.success && verdict.output != () && verdict.output.ok == true {
    complete(#{
        summary: "android-debug-pack " + mode + " passed",
        mode: mode,
        evidence: verdict.output.evidence,
    });
}
let why = "verifier did not accept the run";
if verdict != () && verdict.success && verdict.output != () && verdict.output.evidence != () {
    why = verdict.output.evidence;
}
complete(#{ summary: "android-debug-pack " + mode + " failed", mode: mode, evidence: why });
