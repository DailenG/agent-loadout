//! Links the VC runtime statically into `loadout.exe` on MSVC targets (the
//! UCRT stays dynamic; it ships with Windows), so the CLI does not need the
//! Visual C++ Redistributable. No effect on other targets.

fn main() {
    static_vcruntime::metabuild();
}
