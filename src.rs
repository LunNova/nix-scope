use argh::FromArgs;
use nix::unistd::{Pid, User};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;
use termion::terminal_size;
use users::os::unix::GroupExt;

#[derive(FromArgs, Debug)]
/// Monitor Nix build processes
struct Args {
	/// delay between updates in seconds
	#[argh(option, short = 'd', default = "0.25")]
	delay: f32,

	/// run only once and exit
	#[argh(switch, short = '1')]
	once: bool,
}

fn main() -> io::Result<()> {
	let args: Args = argh::from_env();

	if args.once {
		display_screen()?;
	} else {
		loop {
			display_screen()?;
			sleep(Duration::from_secs_f32(args.delay));
		}
	}

	Ok(())
}

fn display_screen() -> io::Result<()> {
	let (width, height) = terminal_size()?;
	let screen = print_screen();
	let screen = screen
		.iter()
		.take(height as usize)
		.map(|line| {
			format!(
				"{:<width$}",
				line.chars().take(width as usize).collect::<String>(),
				width = width as usize
			)
		})
		.collect::<Vec<String>>()
		.join("\n");

	print!("{}{}{}", termion::clear::All, termion::cursor::Goto(1, 1), screen);
	io::stdout().flush()?;

	Ok(())
}

fn print_screen() -> Vec<String> {
	let mut lines = Vec::new();
	let processes = get_processes();

	lines.push(format!("Nix build summary ({} processes)", processes.len()));
	for (user, (path, pids)) in &processes {
		lines.push(format!("    {:4} → {}", pids.len(), path));
	}
	lines.push("".to_string());
	lines.push(" * * * ".to_string());
	lines.push("".to_string());

	for (user, (path, pids)) in &processes {
		let (info, ps_output) = per_output_infos(user, pids, path);
		lines.push(info);
		lines.extend(ps_output.lines().map(String::from));
	}

	lines
}

fn get_processes() -> HashMap<String, (String, Vec<i32>)> {
	let mut processes = HashMap::new();
	let build_users: std::collections::HashSet<_> = build_users().into_iter().collect();

	if let Ok(output) = Command::new("ps")
		.args(&["-o", "user=,pid=", "-u"])
		.arg(&build_users.into_iter().collect::<Vec<_>>().join(","))
		.output()
	{
		let user_pid_map = String::from_utf8_lossy(&output.stdout)
			.lines()
			.filter_map(|line| {
				let parts: Vec<&str> = line.split_whitespace().collect();
				if parts.len() >= 2 {
					Some((parts[0].to_string(), parts[1].parse::<i32>().ok()?))
				} else {
					None
				}
			})
			.fold(HashMap::new(), |mut map, (user, pid)| {
				map.entry(user).or_insert_with(Vec::new).push(pid);
				map
			});

		for (user, pids) in user_pid_map {
			if !pids.is_empty() {
				let path = get_out_path(&user, pids[0]);
				assert!(!path.is_empty());
				processes.insert(user, (path, pids));
			}
		}
	}

	processes
}

fn build_users() -> Vec<String> {
	users::get_group_by_name("nixbld")
		.map(|group| group.members().iter().map(|u| u.to_string_lossy().into_owned()).collect())
		.unwrap_or_default()
}

fn get_out_path(user: &str, pid: i32) -> String {
	// Try to get out path from /proc environment first
	if let Ok(env_content) = fs::read_to_string(format!("/proc/{}/environ", pid)) {
		let vars: Vec<&str> = env_content.split('\0').collect();
		if let Some(out_var) = vars.iter().find(|v| v.starts_with("out=")) {
			if let Some(out_path) = out_var.strip_prefix("out=") {
				if !out_path.is_empty() {
					return out_path.to_string();
				}
			}
		}
	}

	let build_dir = get_build_dir(user).unwrap_or_else(|_| "(unknown)".to_string());
	get_out_from_env_vars(&build_dir).unwrap_or(build_dir)
}

fn get_build_dir(user: &str) -> io::Result<String> {
	let output = Command::new("sh")
		.arg("-c")
		.arg(format!(
			"find -L /tmp -maxdepth 1 -user {} -exec stat --printf '%Z:%n\\n' '{{}}' ';' | sort -n | tail -n1",
			user
		))
		.output()?;

	let last_line = String::from_utf8_lossy(&output.stdout).lines().last().unwrap_or("").to_string();
	Ok(last_line.split(':').last().unwrap_or("").to_string())
}

fn get_out_from_env_vars(build_dir: &str) -> Option<String> {
	let env_vars = fs::read_to_string(format!("{}/env-vars", build_dir)).ok()?;
	env_vars
		.lines()
		.find(|line| line.starts_with("declare -x out="))
		.and_then(|line| line.split('"').nth(1))
		.map(|s| s.to_string())
}

fn per_output_infos(user: &str, pids: &[i32], path: &str) -> (String, String) {
	let info = format!(":: ({}) → {}", user, path);
	let ps_output = Command::new("ps")
		.args(&["-o", "uid,pid,ppid,stime,time,command", "-U", user])
		.output()
		.map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
		.unwrap_or_default();

	(info, ps_output)
}
