// Stream one Windows process's render audio to stdout as 48 kHz stereo s16le.
// Uses the Windows application loopback capture API.
#include <windows.h>
#include <audioclient.h>
#include <audioclientactivationparams.h>
#include <mmdeviceapi.h>
#include <wrl.h>
#include <cstdio>
#include <cstdlib>
#include <fcntl.h>
#include <io.h>
#include <vector>

using Microsoft::WRL::ComPtr;
using Microsoft::WRL::FtmBase;
using Microsoft::WRL::Make;
using Microsoft::WRL::RuntimeClass;
using Microsoft::WRL::RuntimeClassFlags;
using Microsoft::WRL::ClassicCom;

class Activation final : public RuntimeClass<RuntimeClassFlags<ClassicCom>,
    IActivateAudioInterfaceCompletionHandler, FtmBase> {
public:
    HANDLE done = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    HRESULT status = E_FAIL;
    ComPtr<IAudioClient> client;

    ~Activation() { CloseHandle(done); }

    IFACEMETHODIMP ActivateCompleted(IActivateAudioInterfaceAsyncOperation* operation) override {
        ComPtr<IUnknown> object;
        HRESULT activation;
        status = operation->GetActivateResult(&activation, &object);
        if (SUCCEEDED(status)) status = activation;
        if (SUCCEEDED(status)) status = object.As(&client);
        SetEvent(done);
        return S_OK;
    }
};

static void check(HRESULT status, const char* action) {
    if (FAILED(status)) {
        std::fprintf(stderr, "%s: 0x%08lx\n", action, static_cast<unsigned long>(status));
        std::exit(1);
    }
}

int main(int argc, char** argv) {
    if (argc != 2 || !std::strtoul(argv[1], nullptr, 10)) return 2;
    _setmode(_fileno(stdout), _O_BINARY);
    check(CoInitializeEx(nullptr, COINIT_MULTITHREADED), "COM initialization");
    AUDIOCLIENT_ACTIVATION_PARAMS params{};
    params.ActivationType = AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK;
    params.ProcessLoopbackParams.TargetProcessId = std::strtoul(argv[1], nullptr, 10);
    params.ProcessLoopbackParams.ProcessLoopbackMode = PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE;
    PROPVARIANT variant{};
    variant.vt = VT_BLOB;
    variant.blob.cbSize = sizeof(params);
    variant.blob.pBlobData = reinterpret_cast<BYTE*>(&params);
    auto completed = Make<Activation>();
    ComPtr<IActivateAudioInterfaceAsyncOperation> operation;
    check(ActivateAudioInterfaceAsync(VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, __uuidof(IAudioClient),
        &variant, completed.Get(), &operation), "loopback activation");
    if (WaitForSingleObject(completed->done, 10000) != WAIT_OBJECT_0) return 3;
    check(completed->status, "loopback activation result");
    auto client = completed->client;
    WAVEFORMATEX format{};
    format.wFormatTag = WAVE_FORMAT_PCM;
    format.nChannels = 2;
    format.nSamplesPerSec = 48000;
    format.wBitsPerSample = 16;
    format.nBlockAlign = 4;
    format.nAvgBytesPerSec = format.nSamplesPerSec * format.nBlockAlign;
    check(client->Initialize(AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK |
        AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
        0, 0, &format, nullptr), "48 kHz stereo loopback format");
    HANDLE packet = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    check(client->SetEventHandle(packet), "loopback stream event");
    ComPtr<IAudioCaptureClient> capture;
    check(client->GetService(IID_PPV_ARGS(&capture)), "loopback capture service");
    check(client->Start(), "start loopback");
    std::vector<BYTE> blank;
    while (WaitForSingleObject(packet, 3000) == WAIT_OBJECT_0) {
        UINT32 frames;
        check(capture->GetNextPacketSize(&frames), "read packet size");
        while (frames) {
            BYTE* samples;
            DWORD flags;
            check(capture->GetBuffer(&samples, &frames, &flags, nullptr, nullptr), "capture packet");
            const auto length = frames * format.nBlockAlign;
            if (flags & AUDCLNT_BUFFERFLAGS_SILENT) {
                blank.resize(length, 0);
                samples = blank.data();
            }
            auto written = std::fwrite(samples, 1, length, stdout);
            check(capture->ReleaseBuffer(frames), "release packet");
            if (written != length || std::fflush(stdout) == EOF) return 0;
            check(capture->GetNextPacketSize(&frames), "read packet size");
        }
    }
    std::fprintf(stderr, "Loopback audio timed out\n");
    return 4;
}
