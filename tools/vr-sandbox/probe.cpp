#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <d3d11.h>
#include <wincodec.h>
#include <wrl/client.h>
#include <iostream>
#include <vector>
#include <string>
#include <stdexcept>
#include "vendor/openvr.h"
using Microsoft::WRL::ComPtr;
static void check(HRESULT h) {if(FAILED(h))throw std::runtime_error("HRESULT "+std::to_string((unsigned long)h));}
static void save(const wchar_t *path,ID3D11Device *dev,ID3D11DeviceContext *ctx,ID3D11Texture2D *texture) {
    D3D11_TEXTURE2D_DESC desc;texture->GetDesc(&desc);desc.Usage=D3D11_USAGE_STAGING;desc.BindFlags=0;desc.MiscFlags=0;desc.CPUAccessFlags=D3D11_CPU_ACCESS_READ;
    ComPtr<ID3D11Texture2D> stage;check(dev->CreateTexture2D(&desc,nullptr,&stage));ctx->CopyResource(stage.Get(),texture);
    D3D11_MAPPED_SUBRESOURCE data{};check(ctx->Map(stage.Get(),0,D3D11_MAP_READ,0,&data));
    ComPtr<IWICImagingFactory> factory;check(CoCreateInstance(CLSID_WICImagingFactory,nullptr,CLSCTX_INPROC_SERVER,IID_PPV_ARGS(&factory)));
    ComPtr<IWICStream> stream;check(factory->CreateStream(&stream));check(stream->InitializeFromFilename(path,GENERIC_WRITE));
    ComPtr<IWICBitmapEncoder> encoder;check(factory->CreateEncoder(GUID_ContainerFormatPng,nullptr,&encoder));check(encoder->Initialize(stream.Get(),WICBitmapEncoderNoCache));
    ComPtr<IWICBitmapFrameEncode> frame;check(encoder->CreateNewFrame(&frame,nullptr));check(frame->Initialize(nullptr));check(frame->SetSize(desc.Width,desc.Height));
    auto format=GUID_WICPixelFormat32bppRGBA;check(frame->SetPixelFormat(&format));
    std::vector<unsigned char> rgba(desc.Width*desc.Height*4);
    bool bgra=desc.Format==DXGI_FORMAT_B8G8R8A8_UNORM||desc.Format==DXGI_FORMAT_B8G8R8A8_UNORM_SRGB||desc.Format==DXGI_FORMAT_B8G8R8A8_TYPELESS;
    bool rgb=desc.Format==DXGI_FORMAT_R8G8B8A8_UNORM||desc.Format==DXGI_FORMAT_R8G8B8A8_UNORM_SRGB||desc.Format==DXGI_FORMAT_R8G8B8A8_TYPELESS;
    if(!bgra&&!rgb)throw std::runtime_error("Unsupported mirror format "+std::to_string(desc.Format));
    for(unsigned y=0;y<desc.Height;++y)for(unsigned x=0;x<desc.Width;++x){auto src=(unsigned char*)data.pData+y*data.RowPitch+x*4;auto dst=rgba.data()+(y*desc.Width+x)*4;dst[0]=src[bgra?2:0];dst[1]=src[1];dst[2]=src[bgra?0:2];dst[3]=255;}
    ctx->Unmap(stage.Get(),0);check(frame->WritePixels(desc.Height,desc.Width*4,(UINT)rgba.size(),rgba.data()));check(frame->Commit());check(encoder->Commit());
    std::cout<<"{\"width\":"<<desc.Width<<",\"height\":"<<desc.Height<<",\"format\":"<<desc.Format<<"}";
}
int wmain(int argc,wchar_t **argv) {
    if(argc<2)return 2;
    CoInitializeEx(nullptr,COINIT_MULTITHREADED);
    vr::EVRInitError e;auto system=vr::VR_Init(&e,vr::VRApplication_Background);
    if(e!=vr::VRInitError_None){std::cerr<<"VR_Init: "<<vr::VR_GetVRInitErrorAsEnglishDescription(e)<<" ("<<e<<")\n";return 3;}
    try {
        std::wstring cmd=argv[1];
        if(cmd==L"view") {
            vr::TrackedDevicePose_t poses[64];system->GetDeviceToAbsoluteTrackingPose(vr::TrackingUniverseStanding,0,poses,64);
            std::cout<<"{\"head\":[";for(int a=0;a<3;++a)for(int b=0;b<4;++b){if(a||b)std::cout<<",";std::cout<<poses[0].mDeviceToAbsoluteTracking.m[a][b];}
            std::cout<<"],\"eyes\":[";
            for(int eye=0;eye<2;++eye){if(eye)std::cout<<",";float l,r,t,b;system->GetProjectionRaw((vr::EVREye)eye,&l,&r,&t,&b);auto m=system->GetEyeToHeadTransform((vr::EVREye)eye);
                std::cout<<"{\"projection\":["<<l<<","<<r<<","<<t<<","<<b<<"],\"eye_to_head\":[";for(int a=0;a<3;++a)for(int b=0;b<4;++b){if(a||b)std::cout<<",";std::cout<<m.m[a][b];}std::cout<<"]}";
            }std::cout<<"]}\n";
        } else if(cmd==L"devices") {
            vr::TrackedDevicePose_t poses[64];system->GetDeviceToAbsoluteTrackingPose(vr::TrackingUniverseStanding,0,poses,64);
            std::cout<<"[";bool first=true;
            for(unsigned i=0;i<64;++i)if(system->IsTrackedDeviceConnected(i)){
                if(!first)std::cout<<",";first=false;char model[256]={},type[256]={};system->GetStringTrackedDeviceProperty(i,vr::Prop_ModelNumber_String,model,256);system->GetStringTrackedDeviceProperty(i,vr::Prop_ControllerType_String,type,256);
                auto &m=poses[i].mDeviceToAbsoluteTracking;std::cout<<"{\"index\":"<<i<<",\"class\":"<<system->GetTrackedDeviceClass(i)<<",\"valid\":"<<(poses[i].bPoseIsValid?"true":"false")<<",\"model\":\""<<model<<"\",\"profile\":\""<<type<<"\",\"position\":["<<m.m[0][3]<<","<<m.m[1][3]<<","<<m.m[2][3]<<"]}";
            }std::cout<<"]\n";
        } else if(cmd==L"capture"&&argc==3) {
            ComPtr<ID3D11Device> dev;ComPtr<ID3D11DeviceContext> ctx;D3D_FEATURE_LEVEL level;
            check(D3D11CreateDevice(nullptr,D3D_DRIVER_TYPE_HARDWARE,nullptr,0,nullptr,0,D3D11_SDK_VERSION,&dev,&level,&ctx));
            std::cout<<"[";
            for(int eye=0;eye<2;++eye){ID3D11ShaderResourceView *raw=nullptr;auto result=vr::VRCompositor()->GetMirrorTextureD3D11((vr::EVREye)eye,dev.Get(),(void**)&raw);if(result!=vr::VRCompositorError_None)throw std::runtime_error("GetMirrorTextureD3D11: "+std::to_string(result));
                Sleep(250); // The compositor fills a newly requested mirror on its next frames.
                ComPtr<ID3D11Resource> res;raw->GetResource(&res);ComPtr<ID3D11Texture2D> texture;check(res.As(&texture));
                if(eye)std::cout<<",";auto path=std::wstring(argv[2])+(eye?L"-right.png":L"-left.png");save(path.c_str(),dev.Get(),ctx.Get(),texture.Get());vr::VRCompositor()->ReleaseMirrorTextureD3D11(raw);
            }std::cout<<"]\n";
        } else if(cmd==L"overlays") {
            std::cout<<"[";bool first=true;for(int i=2;i<argc;++i){std::wstring wide=argv[i];std::string key(wide.begin(),wide.end());vr::VROverlayHandle_t h;auto o=vr::VROverlay();if(o->FindOverlay(key.c_str(),&h)!=vr::VROverlayError_None)continue;
                float width=0,alpha=0;o->GetOverlayWidthInMeters(h,&width);o->GetOverlayAlpha(h,&alpha);vr::HmdMatrix34_t m{};vr::ETrackingUniverseOrigin origin;o->GetOverlayTransformAbsolute(h,&origin,&m);
                if(!first)std::cout<<",";first=false;std::cout<<"{\"key\":\""<<key<<"\",\"handle\":"<<h<<",\"visible\":"<<(o->IsOverlayVisible(h)?"true":"false")<<",\"width_m\":"<<width<<",\"alpha\":"<<alpha<<",\"transform\":[";
                for(int a=0;a<3;++a)for(int b=0;b<4;++b){if(a||b)std::cout<<",";std::cout<<m.m[a][b];}std::cout<<"]}";
            }std::cout<<"]\n";
        } else throw std::runtime_error("Unknown probe command");
    }catch(const std::exception &error){std::cerr<<error.what()<<"\n";vr::VR_Shutdown();return 4;}
    vr::VR_Shutdown();return 0;
}
