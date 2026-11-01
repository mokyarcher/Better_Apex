using System.IO;
using System.Security.Cryptography;

namespace BetterApex.Storage;

internal sealed class ImageEvidence
{
    private readonly byte[] _bytes;
    public string OriginalName { get; }
    public string Hash { get; }
    public string Extension { get; }
    public ReadOnlyMemory<byte> Bytes => _bytes;

    private ImageEvidence(string path, byte[] bytes)
    {
        OriginalName = Path.GetFileName(path);
        Extension = Path.GetExtension(path).ToLowerInvariant();
        if (Extension is not (".png" or ".jpg" or ".jpeg" or ".bmp"))
            throw new InvalidDataException("截图格式不受支持。");
        _bytes = bytes;
        Hash = Convert.ToHexString(SHA256.HashData(bytes));
    }

    public static ImageEvidence Capture(string path)
    {
        using var stream = File.OpenRead(path);
        if (stream.Length > 64L * 1024 * 1024)
            throw new InvalidDataException("截图超过 64 MB。");
        using var memory = new MemoryStream();
        stream.CopyTo(memory);
        return new ImageEvidence(path, memory.ToArray());
    }

    public void VerifyUnchanged(string path)
    {
        using var stream = File.OpenRead(path);
        if (Convert.ToHexString(SHA256.HashData(stream)) != Hash)
            throw new InvalidDataException("截图在识别期间发生变化，请重新识别后保存。");
    }
}
