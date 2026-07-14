# Security policy / 安全策略

## Supported versions

Security fixes are provided for the latest released version. The `main` branch
is development code and is not a stable release channel.

## Reporting a vulnerability

Please do **not** open a public issue for a suspected vulnerability, exposed
credential, authentication bypass, or data-loss flaw. Use GitHub's private
vulnerability reporting on the repository **Security** page. If that option is
temporarily unavailable, contact the maintainer through the email shown on the
maintainer's GitHub profile and include `Aeon Memory security` in the subject.

Include affected versions, impact, reproduction steps, and any suggested fix.
You should receive an acknowledgement within seven days. Please allow a
reasonable remediation window before public disclosure.

## Deployment boundary

Aeon Memory stores potentially sensitive conversation history. Keep
`server.host` on `127.0.0.1` unless remote access is required. Before binding a
non-loopback address, set a strong `server.apiKey`, restrict network access,
protect the configuration and data directory, and terminate TLS at a trusted
reverse proxy. Never commit API keys or production memory data.

## 中文说明

疑似漏洞、凭据泄露、鉴权绕过或数据丢失问题请勿提交公开 Issue。请优先使用仓库
Security 页面中的私密漏洞报告功能，并提供受影响版本、影响范围和复现步骤。Aeon Memory
会保存可能敏感的对话记录；非必要不要监听公网地址，远程部署必须启用强 API Key、网络
访问控制与 TLS，并妥善保护配置文件和数据目录。

