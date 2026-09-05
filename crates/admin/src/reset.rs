use std::path::{Path, PathBuf};

use bichon_core::{
    admin::meta::{find_admin, open_database, update_admin_password},
    utils::encrypt::internal_decrypt_string,
};
use console::{style, Emoji};
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Password, Select};

pub fn handle_reset_password(theme: &ColorfulTheme) {
    let root_dir_str: String = Input::with_theme(theme)
        .with_prompt("Enter the absolute path for 'bichon_root_dir'")
        .validate_with(|input: &String| -> Result<(), &str> {
            let path = Path::new(input);
            if !path.is_absolute() {
                return Err("Path must be absolute.");
            }
            if !path.exists() {
                return Err("Directory does not exist.");
            }
            let memdb_dir = path.join("memdb");
            if !memdb_dir.exists() || !memdb_dir.is_dir() {
                return Err("Invalid directory: 'memdb' data directory not found.");
            }
            Ok(())
        })
        .interact_text()
        .unwrap();

    let root_path = PathBuf::from(&root_dir_str);
    let database = open_database(&root_path.join("memdb")).unwrap_or_else(|e| {
        eprintln!(
            "\n{} Failed to open database.",
            style("ERROR:").red().bold()
        );
        eprintln!("Details: {:?}", e);
        std::process::exit(1);
    });

    let admin = find_admin(&database);

    match admin {
        Ok(Some(user)) => {
            println!("\n{}", style("Admin user found:").green().bold());
            println!("----------------------------------------");
            println!("{:<12} : {}", "Username", style(&user.username).cyan());
            println!("{:<12} : {}", "Email", style(&user.email).cyan());
        }
        Ok(None) => {
            println!(
                "\n{}",
                style("ERROR: No admin user found in the database.")
                    .red()
                    .bold()
            );
            println!("Please ensure the system has been initialized correctly.");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!(
                "\n{} Failed to query admin user.",
                style("ERROR:").red().bold()
            );
            eprintln!("Details: {:?}", e);
            std::process::exit(1);
        }
    }

    let encryption_key = loop {
        let auth_methods = vec![
            "Enter encryption password manually",
            "Read from password file",
        ];
        let method = Select::with_theme(theme)
            .with_prompt("How would you like to provide the database encryption key?")
            .items(&auth_methods)
            .interact()
            .unwrap();

        let raw_key = if method == 0 {
            Input::with_theme(theme)
                .with_prompt("Enter Encryption Password")
                .interact()
                .unwrap()
        } else {
            let file_path: String = Input::with_theme(theme)
                .with_prompt("Enter path to encryption password file")
                .interact_text()
                .unwrap();

            match std::fs::read_to_string(&file_path) {
                Ok(content) => content.trim().to_string(),
                Err(e) => {
                    println!("{}: {}", style("Failed to read file").red(), e);
                    continue;
                }
            }
        };

        if raw_key.is_empty() {
            println!("{}", style("Key cannot be empty.").red());
            continue;
        }

        let prompt_message = format!(
            "Encryption key loaded: [ {} ]\n\n  \
                {}: This key must match the database encryption key used by the server.\n  \
                It corresponds to these settings in your service:\n    \
                - Arguments: {} or {}\n    \
                - Envs:      {} or {}\n\n  \
                Do you want to continue?",
            style(&raw_key).cyan().bold(),
            style("IMPORTANT").yellow().bold(),
            style("--bichon_encrypt_password").italic(),
            style("--bichon_encrypt_password_file").italic(),
            style("BICHON_ENCRYPT_PASSWORD").green(),
            style("BICHON_ENCRYPT_PASSWORD_FILE").green()
        );

        if Confirm::with_theme(theme)
            .with_prompt(prompt_message)
            .default(true)
            .interact()
            .unwrap()
        {
            break raw_key;
        }
    };

    let admin = find_admin(&database);

    match admin {
        Ok(Some(user)) => {
            println!("----------------------------------------");
            println!("{:<12} : {}", "Username", style(&user.username).cyan());
            println!("{:<12} : {}", "Email", style(&user.email).cyan());

            let pwd_display = match &user.password {
                Some(p) if p.starts_with("$argon2id$") => {
                    style("(argon2id hash - cannot be displayed)").dim().italic().to_string()
                }
                Some(p) => {
                    let password = match internal_decrypt_string(&encryption_key, p) {
                        Ok(p) => p,
                        Err(e) => {
                            println!("\n{}", style("ERROR: Decryption Failed").red().bold());
                            println!(
                                "{}",
                                style("The provided encryption key is incorrect or invalid for this database.").yellow()
                            );
                            println!(
                                "{} Please verify your {} or the {} you provided.",
                                style("➔").cyan(),
                                style("encryption password").bold(),
                                style("key file").bold()
                            );
                            eprintln!("\nTechnical details: {:?}", e);
                            std::process::exit(1);
                        }
                    };
                    style(password).yellow().to_string()
                }
                None => style("None (No password set)").dim().italic().to_string(),
            };

            println!("{:<12} : {}", "Password", pwd_display);
            println!("----------------------------------------");

            if !dialoguer::Confirm::with_theme(theme)
                .with_prompt(format!(
                    "Do you want to reset the password for '{}'?",
                    user.username
                ))
                .interact()
                .unwrap()
            {
                println!("Operation cancelled.");
                return;
            }
        }
        Ok(None) => {
            println!(
                "\n{}",
                style("ERROR: No admin user found in the database.")
                    .red()
                    .bold()
            );
            println!("Please ensure the system has been initialized correctly.");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!(
                "\n{} Failed to query admin user.",
                style("ERROR:").red().bold()
            );
            eprintln!("Details: {:?}", e);
            std::process::exit(1);
        }
    }

    println!(
        "\n{}",
        style("TARGET: Reset password for user 'admin'")
            .yellow()
            .bold()
    );

    let new_login_password = Password::with_theme(theme)
        .with_prompt("Enter new Admin Login Password")
        .with_confirmation("Repeat password to confirm", "Passwords do not match!")
        .interact()
        .unwrap();

    if !Confirm::with_theme(theme)
        .with_prompt("Proceed with database update?")
        .interact()
        .unwrap()
    {
        return;
    }

    println!("\n{} {}", style("⌛").yellow(), "Updating database...");

    match update_admin_password(&database, new_login_password, &encryption_key) {
        Ok(_) => {
            println!(
                "\n{} {}",
                Emoji("✨", "*"),
                style("Success! Admin password has been updated.")
                    .green()
                    .bold()
            );
            println!(
                "{}",
                style("You can now log in with the new password.").dim()
            );
        }
        Err(e) => {
            println!(
                "\n{}",
                style("ERROR: Failed to update database").red().bold()
            );
            eprintln!(
                "{} Could not save the new password to the database.",
                style("➔").cyan()
            );
            eprintln!("\nDetails: {:?}", e);
            std::process::exit(1);
        }
    }
}
