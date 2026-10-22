# iOS 提醒事项 → WebDAV → NOTE4C

链路：快捷指令读取所选提醒事项清单，生成完整 `todo.json`，用 WebDAV PUT 覆盖同一文件；NOTE4C 定时 GET 该文件，或由 BOOT 短按立即拉取。

本文是配置步骤，尚未在用户的 iPhone / WebDAV 服务上执行。不同 iOS 版本的动作中文名可能略有差异，括号中给出英文名。

## 1. 文件格式

文件名建议 `todo.json`，编码 UTF-8，内容是 JSON 对象：

```json
{
  "version": 1,
  "items": [
    {
      "text": "买牛奶",
      "detail": "下班后去超市",
      "important": true,
      "completed": false
    },
    {
      "text": "提交项目报告",
      "detail": "09-09 14:00"
    }
  ]
}
```

| 字段 | 含义 | 要求 |
|---|---|---|
| `version` | 数据格式版本 | 必须是数字 1，不是小组件版本 |
| `items` | 完整待办快照 | 必须是数组，即使只有一项 |
| `text` | 第一行标题 | 必填字符串，1～512 UTF-8 字节 |
| `detail` | 第二行备注/日期 | 可省略，最长 256 UTF-8 字节 |
| `important` | 红色重要事项 | 可省略，默认 false；使用布尔值 |
| `completed` | 是否完成 | 可省略，默认 false；true 项不显示 |

最多 64 项、整个响应最多 16 KiB；当前屏幕显示前 6 条未完成事项，按数组顺序。建议快捷指令先限制为 20 条，使用简短标题和备注。不要上传图片、附件或整篇长备注。

全部完成时仍要上传下面的有效空快照，才能清除屏幕上的旧事项：

```json
{"version":1,"items":[]}
```

## 2. 先验证 WebDAV 上传

在 WebDAV 服务中先创建 `note4c` 目录，取得完整文件 URL。例如：

```text
https://dav.example.com/服务规定的路径/note4c/todo.json
```

必须是文件的 WebDAV URL，不能是网盘登录页或分享展示页。Nextcloud 的一个例子是：

```text
https://cloud.example.com/remote.php/dav/files/USERNAME/note4c/todo.json
```

先用一个固定 JSON 文本测试上传，再连接真实提醒事项，方便区分认证/URL 问题与数据转换问题。

以下按常见的 HTTPS + Basic 认证说明，具体以服务商为准：

1. 「文本」：填写 `WebDAV用户名:应用专用密码`。
2. 「Base64 编码」（Base64 Encode）：对上面的文本编码，关闭换行。
3. 「文本」：填写 `Basic `，后面插入编码结果变量。`Basic` 后保留一个空格。
4. 「获取 URL 内容」（Get Contents of URL）：
   - URL：完整的 `todo.json` URL。
   - 方法：`PUT`。
   - 请求头：`Authorization` 为上面的 Basic 文本；`Content-Type` 为 `application/json; charset=utf-8`。
   - 请求体选择「文件」，选择生成的 JSON 文本/文件内容；可先用「设定名称」（Set Name）将 JSON 文本命名为 `todo.json`。
5. 再用相同 URL 和 Authorization 做一次 `GET`，将结果交给「从输入获取字典」或「快速查看」，核对 version 和 items。

PUT 的请求体是原始 JSON 文件内容，不是 multipart 表单，也不是整个 JSON 的 Base64 编码。Base64 只用于 Basic 的用户名/密码头。这里的账号是 WebDAV 账号，与设备管理用的 Admin token 无关。

成功创建/覆盖文件时，服务端可能返回空响应；不要以响应为空判断失败。首次通过 GET 读回文件确认。若父目录不存在，先在网盘中创建它。不要把密码放进 URL 或分享包含密码的快捷指令；优先使用服务提供的应用专用密码。

## 3. 从提醒事项生成 JSON

推荐先映射标题和备注，暂时省略 important/completed：已经过滤未完成项目，它们的默认值就够用。

1. 「查找提醒事项」（Find Reminders）：选择一个实际清单，条件为未完成，选择所需排序，限制 20 条。首次运行允许读取提醒事项。
2. 「计数」（Count）：取得项目数量。
3. 「如果」数量为 0：用「文本」输出 `{"version":1,"items":[]}`。
4. 「否则」：
   - 「重复每一项」（Repeat with Each），输入查找到的提醒事项。
   - 在循环内建立「字典」（Dictionary）：`text` 取“重复项目”的标题；`detail` 取备注，或者取格式化后的到期日期。没有备注/日期时省略 detail 或填空串。
   - 在循环内最后使用「从输入获取文本」（Get Text from Input）将该字典转为 JSON 文本，使“重复结果”是一组已编码的 JSON 对象文本。
   - 结束循环后，用「合并文本」（Combine Text），输入“重复结果”，自定义分隔符为一个英文逗号 `,`。
   - 用「文本」包成以下结构，将 `<合并结果>` 替换成真正的魔法变量块，而不是原样文字：

```text
{"version":1,"items":[<合并结果>]}
```

5. 「结束如果」后，将“如果结果”作为最终 JSON 文本，接到第 2 节的 PUT 文件请求体。

先把字典序列化，再合并对象；不要把未经编码的标题/备注直接拼进 JSON。这样引号、换行和中文会得到正确转义。显式的数组括号还能避免单条任务被当成对象，零条任务则由分支生成 `[]`。

第一次用「快速查看」检查 0 条、1 条、多条三种情况，再启用网络上传。需要标红时，再将提醒事项的高优先级规则映射为字典里的布尔值 `important=true`；不要填字符串 `"true"`。

## 4. 自动运行

先确保手动运行快捷指令能上传并读回，再到「快捷指令 → 自动化」添加触发器，例如：

- 关闭“提醒事项”App 时运行，并选择立即运行。
- 每天固定时间运行，可按需要建立多个时间触发器。
- 保留一个主屏幕快捷指令入口，随时手动上传。

关闭 App 的触发器不覆盖 Siri 或其他设备上的所有修改；它不是提醒事项数据库的实时变化通知。锁屏、网络和权限下的实际自动运行需在 iPhone 上试一次。

手机上传和屏幕拉取是两个步骤。先上传云文件，屏幕随后每 5 分钟检查，或者按 BOOT 立即检查；BOOT 不负责让手机运行上传快捷指令。

## 5. 当前小组件的认证边界

当前源码 TODO 0.3.0 已加入 Basic：在新管理页的 TODO 配置中选择 Basic，填写 WebDAV 用户名和密码/应用密码，API 地址填写该 JSON 文件的直接 HTTPS GET 地址。程序生成认证头，不要把 `Basic ...` 填进 Bearer Token 框；JSON 格式无需改变。

旧 0.2.0 包只支持无认证和 HTTPS Bearer Token；Basic 必须同时更新固件与小组件包。设备 HTTPS 默认验证服务器证书，当前没有 NAS 私有 CA 导入界面，自签名证书不能假定可直接使用。Basic 的真实 NAS GET 尚未实机验证。

## 官方参考

- [Apple：快捷指令中的查找与过滤](https://support.apple.com/en-au/guide/shortcuts/apd3c845e881/ios)
- [Apple：字典](https://support.apple.com/en-au/guide/shortcuts/apd43b69f337/ios)
- [Apple：重复动作与重复结果](https://support.apple.com/en-lamr/guide/shortcuts/apdc11deb2c1/ios)
- [Apple：获取 URL 内容支持 PUT、JSON/表单/文件请求体](https://support.apple.com/guide/shortcuts/request-your-first-api-apd58d46713f/ios)
- [Apple：自动化的自动运行设置](https://support.apple.com/en-lamr/guide/shortcuts/apd602971e63/ios)
- [Nextcloud：WebDAV 文件操作与认证](https://docs.nextcloud.com/server/stable/developer_manual/client_apis/WebDAV/basic.html)
