# 内置 Markdown 渲染资源

本目录保存 MD Viewer 离线渲染高级 Markdown 所需的前端资源：

- `mermaid.min.js`：Mermaid 11.16.0 官方 npm 包中的浏览器构建，用于渲染 `mermaid` 围栏代码块。
- `katex.min.js`：KaTeX 0.16.22 官方 npm 包中的浏览器构建，用于渲染行内与块级 LaTeX 公式。
- `katex-inlined.css`：由 KaTeX 0.16.22 的 `dist/katex.min.css` 生成，并内嵌该包中的 20 个 WOFF2 字体，确保离线环境下公式字体完整。

这些资源在编译期嵌入可执行文件，运行时不访问 CDN 或其他网络服务。

Mermaid 和 KaTeX 均采用 MIT 许可证，完整上游声明见根目录 `THIRD_PARTY_NOTICES.md`。

## 来源与完整性

- Mermaid：`mermaid@11.16.0`，npm tarball integrity `sha512-Zvm3kbstgdpvIJPPItlL7fppIZ3kibvc1oZIGxdvk9t6UFz6flv+Jw7FtRGKwfcI8OckmH04LqG6LlS6X4B1pA==`。
- KaTeX：`katex@0.16.22`，npm tarball integrity `sha512-XCHRdUw4lf3SKBaJe4EvgqIuWwkPSo9XoeO8GjQW94Bp7TWv9hNhzZjZ+OH9yf1UmLygb7DIT5GSFQiyt16zYg==`。
- `mermaid.min.js` SHA-256：`74D7C46DABCA328C2294733910A8AA1ED0C37451776E8D5295DA38A2B758FB9B`。
- `katex.min.js` SHA-256：`E8D885505949F3A5F4ABDD5DD0D53696BD1371AD26FFBF4F310DCD77C8CDAE89`。
- `katex-inlined.css` SHA-256：`C87D2B61698D44A07331EE70F61E01EC7AFA1C102903403C03F1F7D44FA4F746`。
