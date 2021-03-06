use super::{
    status::{FAILURE, SUCCESS},
    variables::{Value, Variables},
};
use crate::sys::env as sys_env;
use std::{
    borrow::Cow,
    collections::VecDeque,
    env::{self, set_current_dir},
    path::{Component, Path, PathBuf},
};

fn set_current_dir_ion(dir: &Path) -> Result<(), Cow<'static, str>> {
    set_current_dir(dir).map_err(|why| Cow::Owned(format!("{}", why)))?;

    env::set_var(
        "OLDPWD",
        env::var("PWD")
            .ok()
            .and_then(|pwd| if pwd.is_empty() { None } else { Some(pwd) })
            .unwrap_or_else(|| "?".into()),
    );

    env::set_var("PWD", dir.to_str().unwrap_or("?"));
    Ok(())
}

#[derive(Debug)]
pub struct DirectoryStack {
    dirs: VecDeque<PathBuf>, // The top is always the current directory
}

impl DirectoryStack {
    fn normalize_path(&mut self, dir: &str) -> PathBuf {
        // Create a clone of the current directory.
        let mut new_dir = match self.dirs.front() {
            Some(cur_dir) => cur_dir.clone(),
            None => PathBuf::new(),
        };

        // Iterate through components of the specified directory
        // and calculate the new path based on them.
        for component in Path::new(dir).components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    new_dir.pop();
                }
                _ => {
                    new_dir.push(component);
                }
            };
        }

        new_dir
    }

    // pushd -<num>
    fn rotate_right(&mut self, num: usize) {
        let len = self.dirs.len();
        self.rotate_left(len - (num % len));
    }

    // pushd +<num>
    fn rotate_left(&mut self, num: usize) {
        let cloned = self.dirs.clone();
        for (dest, src) in self.dirs.iter_mut().zip(cloned.iter().cycle().skip(num)) {
            *dest = src.clone();
        }
    }

    // sets current_dir to the element referred by index
    fn set_current_dir_by_index(
        &self,
        index: usize,
        caller: &str,
    ) -> Result<(), Cow<'static, str>> {
        let dir = self.dirs.get(index).ok_or_else(|| {
            Cow::Owned(format!("ion: {}: {}: directory stack out of range", caller, index))
        })?;

        set_current_dir_ion(dir)
    }

    fn print_dirs(&self) {
        let dir = self.dirs.iter().fold(String::new(), |acc, dir| {
            acc + " " + dir.to_str().unwrap_or("ion: no directory found")
        });
        println!("{}", dir.trim_start());
    }

    pub(crate) fn dir_from_bottom(&self, num: usize) -> Option<&PathBuf> {
        self.dirs.iter().rev().nth(num)
    }

    pub(crate) fn dir_from_top(&self, num: usize) -> Option<&PathBuf> { self.dirs.get(num) }

    pub(crate) fn dirs<'a, I: Iterator<Item = &'a T>, T: 'a + AsRef<str>>(
        &mut self,
        args: I,
    ) -> i32 {
        let mut clear = false; // -c
        let mut abs_pathnames = false; // -l
        let mut multiline = false; // -p | -v
        let mut index = false; // -v

        let mut num_arg = None;

        for arg in args.skip(1).map(AsRef::as_ref) {
            match arg {
                "-c" => clear = true,
                "-l" => abs_pathnames = true,
                "-p" => multiline = true,
                "-v" => {
                    index = true;
                    multiline = true;
                }
                arg => num_arg = Some(arg),
            }
        }

        if clear {
            self.dirs.truncate(1);
        }

        let mapper: fn((usize, &PathBuf)) -> Cow<str> = match (abs_pathnames, index) {
            // ABS, INDEX
            (true, true) => |(num, x)| Cow::Owned(format!(" {}  {}", num, try_abs_path(x))),
            (true, false) => |(_, x)| try_abs_path(x),
            (false, true) => |(num, x)| Cow::Owned(format!(" {}  {}", num, x.to_string_lossy())),
            (false, false) => |(_, x)| x.to_string_lossy(),
        };

        let mut iter = self.dirs.iter().enumerate().map(mapper);

        if let Some(arg) = num_arg {
            let num = match parse_numeric_arg(arg) {
                Some((true, num)) => num,
                Some((false, num)) if self.dirs.len() > num => self.dirs.len() - num - 1,
                _ => return FAILURE, /* Err(Cow::Owned(format!("ion: dirs: {}: invalid
                                      * argument\n", arg))) */
            };
            match iter.nth(num) {
                Some(x) => {
                    println!("{}", x);
                    SUCCESS
                }
                None => FAILURE,
            }
        } else {
            let folder: fn(String, Cow<str>) -> String =
                if multiline { |x, y| x + "\n" + &y } else { |x, y| x + " " + &y };

            if let Some(x) = iter.next() {
                println!("{}", iter.fold(x.to_string(), folder));
            }
            SUCCESS
        }
    }

    fn insert_dir(&mut self, index: usize, path: PathBuf, variables: &Variables) {
        self.dirs.insert(index, path);
        self.dirs.truncate(DirectoryStack::get_size(variables));
    }

    fn push_dir(&mut self, path: PathBuf, variables: &Variables) {
        self.dirs.push_front(path);

        self.dirs.truncate(DirectoryStack::get_size(variables));
    }

    pub(crate) fn change_and_push_dir(
        &mut self,
        dir: &str,
        variables: &Variables,
    ) -> Result<(), Cow<'static, str>> {
        let new_dir = self.normalize_path(dir);
        match set_current_dir_ion(&new_dir) {
            Ok(()) => {
                self.push_dir(new_dir, variables);
                Ok(())
            }
            Err(err) => Err(Cow::Owned(format!(
                "ion: failed to set current dir to {}: {}",
                new_dir.to_string_lossy(),
                err
            ))),
        }
    }

    fn get_previous_dir(&self) -> Option<String> {
        env::var("OLDPWD").ok().filter(|pwd| !pwd.is_empty() && pwd != "?")
    }

    fn switch_to_previous_directory(
        &mut self,
        variables: &Variables,
    ) -> Result<(), Cow<'static, str>> {
        self.get_previous_dir()
            .ok_or(Cow::Borrowed("ion: no previous directory to switch to"))
            .and_then(|prev| {
                self.dirs.remove(0);
                println!("{}", prev);
                self.change_and_push_dir(&prev, variables)
            })
    }

    fn switch_to_home_directory(&mut self, variables: &Variables) -> Result<(), Cow<'static, str>> {
        sys_env::home_dir().map_or(
            Err(Cow::Borrowed("ion: failed to get home directory")),
            |home| {
                home.to_str().map_or(
                    Err(Cow::Borrowed("ion: failed to convert home directory to str")),
                    |home| self.change_and_push_dir(home, variables),
                )
            },
        )
    }

    pub(crate) fn cd(
        &mut self,
        args: &[small::String],
        variables: &Variables,
    ) -> Result<(), Cow<'static, str>> {
        match args.get(1) {
            Some(dir) => {
                if let Some(Value::Array(cdpath)) = variables.get_ref("CDPATH") {
                    let dir = dir.as_ref();
                    if dir == "-" {
                        self.switch_to_previous_directory(variables)
                    } else {
                        let check_cdpath_first = cdpath
                            .iter()
                            .map(|path| {
                                let path_dir = [path, "/", dir].concat();
                                self.change_and_push_dir(&path_dir, variables)
                            })
                            .find(Result::is_ok)
                            .unwrap_or_else(|| self.change_and_push_dir(dir, variables));
                        self.dirs.remove(1);
                        check_cdpath_first
                    }
                } else {
                    unreachable!();
                }
            }
            None => self.switch_to_home_directory(variables),
        }
    }

    pub(crate) fn pushd<T>(
        &mut self,
        args: &[T],
        variables: &mut Variables,
    ) -> Result<(), Cow<'static, str>>
    where
        T: AsRef<str>,
    {
        enum Action {
            Switch,          // <no arguments>
            RotLeft(usize),  // +[num]
            RotRight(usize), // -[num]
            Push(PathBuf),   // [dir]
        }

        let mut keep_front = false; // whether the -n option is present
        let mut action = Action::Switch;

        for arg in args.iter().skip(1).map(AsRef::as_ref) {
            if arg == "-n" {
                keep_front = true;
            } else if let Action::Switch = action {
                // if action is not yet defined
                action = match parse_numeric_arg(arg) {
                    Some((true, num)) => Action::RotLeft(num),
                    Some((false, num)) => Action::RotRight(num),
                    None => Action::Push(PathBuf::from(arg)), // no numeric arg => `dir`-parameter
                };
            } else {
                return Err(Cow::Borrowed("ion: pushd: too many arguments"));
            }
        }

        let len = self.dirs.len();
        match action {
            Action::Switch => {
                if len < 2 {
                    return Err(Cow::Borrowed("ion: pushd: no other directory"));
                }
                if !keep_front {
                    self.set_current_dir_by_index(1, "pushd")?;
                    self.dirs.swap(0, 1);
                }
            }
            Action::RotLeft(num) => {
                if !keep_front {
                    self.set_current_dir_by_index(num, "pushd")?;
                    self.rotate_left(num);
                }
            }
            Action::RotRight(num) => {
                if !keep_front {
                    self.set_current_dir_by_index(len - (num % len), "pushd")?;
                    self.rotate_right(num);
                }
            }
            Action::Push(dir) => {
                let index = if keep_front { 1 } else { 0 };
                let new_dir = self.normalize_path(dir.to_str().unwrap());
                self.insert_dir(index, new_dir, variables);
                self.set_current_dir_by_index(index, "pushd")?;
            }
        };

        self.print_dirs();
        Ok(())
    }

    /// Attempts to set the current directory to the directory stack's previous directory,
    /// and then removes the front directory from the stack.
    pub(crate) fn popd<T: AsRef<str>>(&mut self, args: &[T]) -> Result<(), Cow<'static, str>> {
        let len = self.dirs.len();
        if len <= 1 {
            return Err(Cow::Borrowed("ion: popd: directory stack empty"));
        }

        let mut keep_front = false; // whether the -n option is present
        let mut index: usize = 0;

        for arg in args.iter().skip(1).map(AsRef::as_ref) {
            if arg == "-n" {
                keep_front = true;
            } else {
                let (count_from_front, num) = parse_numeric_arg(arg)
                    .ok_or_else(|| Cow::Owned(format!("ion: popd: {}: invalid argument", arg)))?;

                index = if count_from_front {
                    // <=> input number is positive
                    num
                } else {
                    (len - 1).checked_sub(num).ok_or_else(|| {
                        Cow::Owned(
                            "ion: popd: negative directory stack index out of range".to_owned(),
                        )
                    })?
                };
            }
        }

        // apply -n
        if index == 0 && keep_front {
            index = 1;
        } else if index == 0 {
            // change to new directory, return if not possible
            self.set_current_dir_by_index(1, "popd")?;
        }

        // pop element
        if self.dirs.remove(index).is_some() {
            self.print_dirs();
            Ok(())
        } else {
            Err(Cow::Owned(format!("ion: popd: {}: directory stack index out of range", index)))
        }
    }

    /// This function will take a map of variables as input and attempt to parse the value of
    /// the
    /// directory stack size variable. If it succeeds, it will return the value of that
    /// variable,
    /// else it will return a default value of 1000.
    fn get_size(variables: &Variables) -> usize {
        variables.get_str_or_empty("DIRECTORY_STACK_SIZE").parse::<usize>().unwrap_or(1000)
    }

    /// Create a new `DirectoryStack` containing the current working directory,
    /// if available.
    pub(crate) fn new() -> DirectoryStack {
        let mut dirs: VecDeque<PathBuf> = VecDeque::new();
        match env::current_dir() {
            Ok(curr_dir) => {
                env::set_var("PWD", curr_dir.to_str().unwrap_or_else(|| "?"));
                dirs.push_front(curr_dir);
            }
            Err(_) => {
                eprintln!("ion: failed to get current directory when building directory stack");
                env::set_var("PWD", "?");
            }
        }
        DirectoryStack { dirs }
    }
}

// parses -N or +N patterns
// required for popd, pushd, dirs
fn parse_numeric_arg(arg: &str) -> Option<(bool, usize)> {
    match arg.chars().nth(0) {
        Some('+') => Some(true),
        Some('-') => Some(false),
        _ => None,
    }
    .and_then(|b| arg[1..].parse::<usize>().ok().map(|num| (b, num)))
}

// converts pbuf to an absolute path if possible
fn try_abs_path(pbuf: &PathBuf) -> Cow<str> {
    Cow::Owned(pbuf.canonicalize().unwrap_or_else(|_| pbuf.clone()).to_string_lossy().to_string())
}
