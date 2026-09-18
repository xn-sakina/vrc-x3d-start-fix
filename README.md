<p align="center">
  <img src="assets/app-icon.png" alt="VRChat X3D Start Fix logo" width="95">
</p>

<h1 align="center">VRChat X3D Start Fix</h1>

<p align="center">
  A tiny, portable Windows utility that makes the VRChat startup crash less likely.
</p>

## Download

[Download the latest release]()

Download the app and open it. There is nothing to install or set up.

## What it does

VRChat can sometimes fail during startup with **`Fatal Error in GC`** and **`SuspendThread Loop failed`**. Repeated launch failures are frustrating, so this app provides a simple temporary workaround.

It runs quietly in the background, watches for VRChat, and briefly adds controlled CPU activity while the game starts. This changes CPU timing and scheduling, which can reduce the chance of the startup error. Memory use is minimal, and you do not need to manage it after opening it.

> [!NOTE]
> **Want to know why this happens and how the workaround helps?**
>
> Read the [full startup failure analysis](./analysis/REPORT.md).

### Important

This is a workaround, not a permanent fix. If VRChat fixes the underlying startup issue, you can simply delete this app.

### Languages

The app supports English and Simplified Chinese.

### Why X3D is in the name

The app is called X3D Start Fix because this issue appears more often on X3D CPUs, not because it only works with them. If your CPU is not an X3D model and you see the same startup error, you can still use the app. It is compatible with all CPUs.

## Troubleshooting

If VRChat still fails often, open the tray menu and set **Startup fix intensity** to **Maximum**, then try again.

If the problem continues, please open an issue and share what happened.

## Contributing

The app may still have rough edges. Contributions are welcome, so feel free to open a pull request.

## Thanks and inspiration

This tool exists because people on the official VRChat Feedback forum shared that adding CPU load during startup could reduce the crash rate. I only turned that workaround into a small utility. Credit for discovering and sharing the method belongs to the community members who found it first.

## Disclaimer

This software is provided for learning and research only. It is an independent, unofficial workaround and is not affiliated with or endorsed by VRChat. It does not guarantee that VRChat will start successfully. Use it at your own risk. The authors are not responsible for any damage, data loss, account issues, hardware problems, or other consequences caused by its use.

## License

Licensed under the [MIT License](LICENSE).
