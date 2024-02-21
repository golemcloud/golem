FROM nginx:latest

COPY /gateway/golem-services.conf.template /etc/nginx/nginx.conf.template

EXPOSE 80

CMD ["nginx", "-g", "daemon off;"]