use std::path::PathBuf;

fn main() {
    if let Err(error) = run() {
        eprintln!("导入未完成：{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut source: Option<PathBuf> = None;
    let mut destination: Option<PathBuf> = None;
    let mut dry_run = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--from" => source = Some(args.next().ok_or("--from 缺少旧目录")?.into()),
            "--to" => destination = Some(args.next().ok_or("--to 缺少新目录")?.into()),
            "--dry-run" => dry_run = true,
            "--help" | "-h" => {
                println!("relay-migrate --from OLD_DATA --to NEW_ABSENT_DIRECTORY [--dry-run]\n请先正常停止旧服务并备份。只导入已完成内容；新目录必须不存在，父目录必须存在。\n旧授权和未完成任务不迁移。--dry-run 会核对源文件 SHA，不创建新数据目录。");
                return Ok(());
            }
            _ => return Err(format!("未知参数：{arg}").into()),
        }
    }
    let report = relay_migrate::import_legacy(
        &source.ok_or("缺少 --from")?,
        &destination.ok_or("缺少 --to")?,
        dry_run,
    )?;
    println!(
        "{}：文件 {}，文字 {}，反馈 {}；保留在旧目录的未完成任务 {}。",
        if dry_run {
            "检查通过"
        } else {
            "导入完成"
        },
        report.files,
        report.texts,
        report.feedback,
        report.incomplete_uploads_not_imported
    );
    if report.feedback_converted_to_text > 0 {
        println!(
            "其中 {} 条旧反馈按转换计划保留为文字；原引用保存在导入报告。",
            report.feedback_converted_to_text
        );
    }
    for warning in report.warnings {
        println!("说明：{warning}");
    }
    Ok(())
}
