use std::env;

fn main() {
    println!("cargo:rerun-if-changed=assets/icon-dark.ico");
    println!("cargo:rerun-if-changed=build.rs");

    // Rien à faire hors Windows : les icônes macOS/Linux sont gérées
    // par le bundle ou au runtime.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let version = env!("CARGO_PKG_VERSION");

    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon-dark.ico");
    res.set("FileDescription", "Scanner n8n");
    res.set("ProductName", "Scanner n8n");
    res.set("ProductVersion", version);
    res.set("FileVersion", version);
    res.set("OriginalFilename", "scanner_n8n.exe");
    res.set("InternalName", "scanner_n8n");
    res.set("LegalCopyright", "MIT License");

    if let Err(err) = res.compile() {
        // On n'échoue pas le build : l'application fonctionne même sans
        // ressources Windows, seule l'icône de l'exécutable manquerait.
        println!("cargo:warning=Impossible d'embarquer les ressources Windows : {err}");
    }
}
