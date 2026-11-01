# 离线识别模型

下载日期：2026-10-30。模型随程序复制，运行时不请求网络。

| 文件 | 来源 | SHA-256 |
| --- | --- | --- |
| eng.traineddata | [Tesseract tessdata_best](https://github.com/tesseract-ocr/tessdata_best/blob/main/eng.traineddata) | 8280AED0782FE27257A68EA10FE7EF324CA0F8D85BD2FD145D1C2B560BCB66BA |
| chi_sim.traineddata | [Tesseract tessdata_fast](https://github.com/tesseract-ocr/tessdata_fast/blob/main/chi_sim.traineddata) | A5FCB6F0DB1E1D6D8522F39DB4E848F05984669172E584E8D76B6B3141E1F730 |

两份模型的许可为 Apache-2.0，许可原文见本目录 LICENSE。

.NET 接口使用 [charlesw/tesseract](https://github.com/charlesw/tesseract) 的 Tesseract 5.2.0 NuGet 包（Apache-2.0）。其 Windows 原生运行库依赖 Visual C++ 2019 兼容运行库；当前开发机器已成功加载。未附加安装或修改系统配置。
